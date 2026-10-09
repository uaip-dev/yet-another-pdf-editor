//! PDF engine: owns PDFium on a single dedicated thread.
//!
//! PDFium is not thread-safe, so every operation is sent as a `Job` to the
//! worker thread, which replies over a oneshot channel. This also gives us a
//! natural place to drop stale render jobs later.

use pdfium_render::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use tokio::sync::oneshot;

pub type DocId = u32;

/// Error code the UI recognises to show a password prompt.
pub const ERR_PASSWORD: &str = "PASSWORD_REQUIRED";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    /// Page size in PDF points (1/72 inch), after page rotation.
    pub width: f32,
    pub height: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocInfo {
    pub id: DocId,
    pub path: String,
    pub title: Option<String>,
    pub pages: Vec<PageInfo>,
}

/// Raw RGBA bitmap: 8-byte header (width u32 LE, height u32 LE) + pixels.
pub type RenderedPage = Vec<u8>;

type Reply<T> = oneshot::Sender<Result<T, String>>;

enum Job {
    Open { path: String, password: Option<String>, reply: Reply<DocInfo> },
    Close { id: DocId },
    Render { id: DocId, page: u16, width: u32, reply: Reply<RenderedPage> },
}

pub struct Engine {
    tx: Sender<Job>,
}

impl Engine {
    /// Starts the worker thread. `lib_dirs` are searched in order for the
    /// PDFium shared library before falling back to the system library.
    pub fn start(lib_dirs: Vec<PathBuf>) -> Self {
        let (tx, rx) = channel();
        thread::Builder::new()
            .name("pdfium".into())
            .spawn(move || worker(rx, lib_dirs))
            .expect("failed to spawn pdfium thread");
        Self { tx }
    }

    pub async fn open(&self, path: String, password: Option<String>) -> Result<DocInfo, String> {
        self.call(|reply| Job::Open { path, password, reply }).await
    }

    pub fn close(&self, id: DocId) {
        let _ = self.tx.send(Job::Close { id });
    }

    pub async fn render(&self, id: DocId, page: u16, width: u32) -> Result<RenderedPage, String> {
        self.call(|reply| Job::Render { id, page, width, reply }).await
    }

    async fn call<T>(&self, job: impl FnOnce(Reply<T>) -> Job) -> Result<T, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(job(reply)).map_err(|_| "PDF engine stopped".to_string())?;
        rx.await.map_err(|_| "PDF engine stopped".to_string())?
    }
}

fn bind(lib_dirs: &[PathBuf]) -> Result<Box<dyn PdfiumLibraryBindings>, String> {
    let name = Pdfium::pdfium_platform_library_name();
    for dir in lib_dirs {
        let candidate = dir.join(&name);
        if candidate.exists() {
            if let Ok(b) = Pdfium::bind_to_library(candidate.to_string_lossy().as_ref()) {
                return Ok(b);
            }
        }
    }
    Pdfium::bind_to_system_library()
        .map_err(|e| format!("Could not load PDFium library ({name:?}): {e:?}"))
}

fn err(e: PdfiumError) -> String {
    match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            ERR_PASSWORD.to_string()
        }
        other => format!("{other:?}"),
    }
}

fn worker(rx: Receiver<Job>, lib_dirs: Vec<PathBuf>) {
    let pdfium = match bind(&lib_dirs) {
        Ok(b) => Pdfium::new(b),
        Err(e) => {
            log::error!("{e}");
            // Keep answering so the UI shows the error instead of hanging.
            for job in rx {
                match job {
                    Job::Open { reply, .. } => drop(reply.send(Err(e.clone()))),
                    Job::Render { reply, .. } => drop(reply.send(Err(e.clone()))),
                    Job::Close { .. } => {}
                }
            }
            return;
        }
    };

    let mut docs: HashMap<DocId, PdfDocument> = HashMap::new();
    let mut next_id: DocId = 1;

    for job in rx {
        match job {
            Job::Open { path, password, reply } => {
                // pdfium-render ties the document's lifetime to the password
                // borrow, though PDFium copies it. Leaking a few bytes per
                // password-protected open satisfies the borrow checker.
                let password: Option<&'static str> =
                    password.map(|p| &*Box::leak(p.into_boxed_str()));
                let result = pdfium
                    .load_pdf_from_file(Path::new(&path), password)
                    .map_err(err)
                    .map(|doc| {
                        let info = doc_info(next_id, &path, &doc);
                        docs.insert(next_id, doc);
                        next_id += 1;
                        info
                    });
                let _ = reply.send(result);
            }
            Job::Close { id } => {
                docs.remove(&id);
            }
            Job::Render { id, page, width, reply } => {
                let result = match docs.get(&id) {
                    Some(doc) => render(doc, page, width),
                    None => Err(format!("No open document with id {id}")),
                };
                let _ = reply.send(result);
            }
        }
    }
}

fn doc_info(id: DocId, path: &str, doc: &PdfDocument) -> DocInfo {
    let title = doc
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)
        .map(|t| t.value().trim().to_string())
        .filter(|t| !t.is_empty());
    let pages = doc
        .pages()
        .iter()
        .map(|p| PageInfo { width: p.width().value, height: p.height().value })
        .collect();
    DocInfo { id, path: path.to_string(), title, pages }
}

fn render(doc: &PdfDocument, index: u16, width: u32) -> Result<RenderedPage, String> {
    let page = doc.pages().get(index).map_err(err)?;
    let config = PdfRenderConfig::new()
        .set_target_width(width.clamp(16, 8192) as i32)
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page.render_with_config(&config).map_err(err)?;
    let (w, h) = (bitmap.width() as u32, bitmap.height() as u32);
    let rgba = bitmap.as_rgba_bytes();
    let mut out = Vec::with_capacity(8 + rgba.len());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&rgba);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the corpus from `scripts/make-test-pdfs.py`; skips if it is missing.
    fn corpus(name: &str) -> Option<String> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-pdfs").join(name);
        p.exists().then(|| p.to_string_lossy().into_owned())
    }

    fn engine() -> Engine {
        Engine::start(vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("pdfium")])
    }

    #[tokio::test]
    async fn opens_and_renders() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        assert_eq!(doc.pages.len(), 200);
        assert_eq!(doc.title.as_deref(), Some("Multi-page test document"));
        assert!(doc.pages[3].width > doc.pages[3].height, "page 4 is landscape");

        let bytes = e.render(doc.id, 0, 600).await.unwrap();
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(w, 600);
        assert_eq!(h, (600.0 * 842.0 / 595.0_f32).round() as u32);
        assert_eq!(bytes.len(), 8 + (w * h * 4) as usize);

        e.close(doc.id);
        assert!(e.render(doc.id, 0, 600).await.is_err());
    }

    #[tokio::test]
    async fn password_flow() {
        let Some(path) = corpus("encrypted-password-test.pdf") else { return };
        let e = engine();
        assert_eq!(e.open(path.clone(), None).await.err().as_deref(), Some(ERR_PASSWORD));
        assert_eq!(e.open(path.clone(), Some("nope".into())).await.err().as_deref(), Some(ERR_PASSWORD));
        assert_eq!(e.open(path, Some("test".into())).await.unwrap().pages.len(), 1);
    }
}
