//! PDF engine: owns PDFium on a single dedicated thread.
//!
//! PDFium is not thread-safe, so every operation is a closure sent to the
//! worker thread, which runs it against the open documents and replies over a
//! oneshot channel.
//!
//! All coordinates handed to the UI are in PDF points with a top-left origin,
//! in the page's displayed orientation (after /Rotate and the crop box).

use pdfium_render::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use tokio::sync::oneshot;

pub type DocId = u32;

/// Error code the UI recognises to show a password prompt.
pub const ERR_PASSWORD: &str = "PASSWORD_REQUIRED";

const MAX_OUTLINE_ITEMS: usize = 20_000;
const MAX_SEARCH_HITS: usize = 10_000;

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

/// Text of one page, one entry per PDFium character.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageText {
    /// Unicode code points. Generated characters (spaces, line breaks) are included.
    pub codes: Vec<u32>,
    /// Four numbers per character: left, top, right, bottom. All zero if unknown.
    pub boxes: Vec<f32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// left, top, right, bottom
    pub rect: [f32; 4],
    /// Target page for internal links.
    pub page: Option<u16>,
    /// Target URI for external links.
    pub uri: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u16>,
    pub children: Vec<OutlineItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: u16,
    /// Index into `PageText::codes`.
    pub start: u32,
    pub len: u32,
}

/// Maps PDF user space to top-left, rotated page space.
#[derive(Clone, Copy)]
struct Geom {
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    rot: u8, // quarter turns clockwise
}

impl Geom {
    fn of(page: &PdfPage) -> Self {
        let rect = page
            .boundaries()
            .crop()
            .or_else(|_| page.boundaries().media())
            .map(|b| b.bounds)
            .ok();
        let rot = match page.rotation() {
            Ok(PdfPageRenderRotation::Degrees90) => 1,
            Ok(PdfPageRenderRotation::Degrees180) => 2,
            Ok(PdfPageRenderRotation::Degrees270) => 3,
            _ => 0,
        };
        match rect {
            Some(r) => Geom {
                left: r.left().value,
                top: r.top().value,
                w: r.width().value,
                h: r.height().value,
                rot,
            },
            None => {
                let (w, h) = (page.width().value, page.height().value);
                let (w, h) = if rot % 2 == 1 { (h, w) } else { (w, h) };
                Geom { left: 0.0, top: h, w, h, rot }
            }
        }
    }

    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let (u, v) = (x - self.left, self.top - y);
        match self.rot {
            1 => (self.h - v, u),
            2 => (self.w - u, self.h - v),
            3 => (v, self.w - u),
            _ => (u, v),
        }
    }

    /// Returns [left, top, right, bottom] in page space.
    fn rect(&self, r: &PdfRect) -> [f32; 4] {
        let (ax, ay) = self.point(r.left().value, r.top().value);
        let (bx, by) = self.point(r.right().value, r.bottom().value);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }
}

struct OpenDoc<'a> {
    doc: PdfDocument<'a>,
    text: HashMap<u16, Arc<PageText>>,
}

impl OpenDoc<'_> {
    fn page_text(&mut self, index: u16) -> Result<Arc<PageText>, String> {
        if let Some(t) = self.text.get(&index) {
            return Ok(t.clone());
        }
        let page = self.doc.pages().get(index).map_err(err)?;
        let t = Arc::new(extract_text(&page)?);
        self.text.insert(index, t.clone());
        Ok(t)
    }
}

pub struct Docs<'a> {
    pdfium: &'a Pdfium,
    open: HashMap<DocId, OpenDoc<'a>>,
    next_id: DocId,
}

impl<'a> Docs<'a> {
    fn get(&mut self, id: DocId) -> Result<&mut OpenDoc<'a>, String> {
        self.open.get_mut(&id).ok_or_else(|| format!("No open document with id {id}"))
    }
}

type Task = Box<dyn for<'a> FnOnce(Result<&mut Docs<'a>, &str>) + Send>;

pub struct Engine {
    tx: Sender<Task>,
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

    /// Runs `f` on the PDFium thread and returns its result.
    async fn run<T, F>(&self, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&mut Docs<'a>) -> Result<T, String> + Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        let task: Task = Box::new(move |docs| {
            let _ = reply.send(docs.map_err(str::to_string).and_then(f));
        });
        self.tx.send(task).map_err(|_| "PDF engine stopped".to_string())?;
        rx.await.map_err(|_| "PDF engine stopped".to_string())?
    }

    pub async fn open(&self, path: String, password: Option<String>) -> Result<DocInfo, String> {
        // pdfium-render ties the document's lifetime to the password borrow,
        // though PDFium copies it. Leaking a few bytes per password-protected
        // open satisfies the borrow checker.
        let password: Option<&'static str> = password.map(|p| &*Box::leak(p.into_boxed_str()));
        self.run(move |docs| {
            let doc = docs.pdfium.load_pdf_from_file(Path::new(&path), password).map_err(err)?;
            let id = docs.next_id;
            docs.next_id += 1;
            let info = doc_info(id, &path, &doc);
            docs.open.insert(id, OpenDoc { doc, text: HashMap::new() });
            Ok(info)
        })
        .await
    }

    pub async fn close(&self, id: DocId) {
        let _ = self.run(move |docs| Ok(docs.open.remove(&id).map(drop))).await;
    }

    pub async fn render(&self, id: DocId, page: u16, width: u32) -> Result<RenderedPage, String> {
        self.run(move |docs| render(&docs.get(id)?.doc, page, width)).await
    }

    pub async fn page_text(&self, id: DocId, page: u16) -> Result<Arc<PageText>, String> {
        self.run(move |docs| docs.get(id)?.page_text(page)).await
    }

    pub async fn links(&self, id: DocId, page: u16) -> Result<Vec<Link>, String> {
        self.run(move |docs| links(&docs.get(id)?.doc, page)).await
    }

    pub async fn outline(&self, id: DocId) -> Result<Vec<OutlineItem>, String> {
        self.run(move |docs| {
            let doc = &docs.get(id)?.doc;
            let mut budget = MAX_OUTLINE_ITEMS;
            Ok(outline_level(doc.bookmarks().root(), &mut budget, 0))
        })
        .await
    }

    pub async fn search(&self, id: DocId, query: String, match_case: bool) -> Result<Vec<SearchHit>, String> {
        self.run(move |docs| {
            let doc = docs.get(id)?;
            let needle: Vec<u32> = query.chars().map(|c| fold(c as u32, match_case)).collect();
            let mut hits = Vec::new();
            if needle.is_empty() {
                return Ok(hits);
            }
            for page in 0..doc.doc.pages().len() {
                let text = doc.page_text(page)?;
                let hay: Vec<u32> = text.codes.iter().map(|&c| fold(c, match_case)).collect();
                let mut i = 0;
                while i + needle.len() <= hay.len() {
                    if hay[i..i + needle.len()] == needle[..] {
                        hits.push(SearchHit { page, start: i as u32, len: needle.len() as u32 });
                        if hits.len() >= MAX_SEARCH_HITS {
                            return Ok(hits);
                        }
                        i += needle.len();
                    } else {
                        i += 1;
                    }
                }
            }
            Ok(hits)
        })
        .await
    }
}

/// Case-folds a code point (1:1, so indices stay aligned) and treats
/// generated line breaks and tabs as spaces.
fn fold(c: u32, match_case: bool) -> u32 {
    let ch = char::from_u32(c).unwrap_or(' ');
    let ch = if matches!(ch, '\r' | '\n' | '\t' | '\u{a0}') { ' ' } else { ch };
    if match_case {
        ch as u32
    } else {
        ch.to_lowercase().next().unwrap_or(ch) as u32
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

fn worker(rx: Receiver<Task>, lib_dirs: Vec<PathBuf>) {
    let pdfium = match bind(&lib_dirs) {
        Ok(b) => Pdfium::new(b),
        Err(e) => {
            log::error!("{e}");
            // Keep answering so the UI shows the error instead of hanging.
            for task in rx {
                task(Err(&e));
            }
            return;
        }
    };
    let mut docs = Docs { pdfium: &pdfium, open: HashMap::new(), next_id: 1 };
    for task in rx {
        task(Ok(&mut docs));
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

fn extract_text(page: &PdfPage) -> Result<PageText, String> {
    let geom = Geom::of(page);
    let text = page.text().map_err(err)?;
    let chars = text.chars();
    let mut codes = Vec::with_capacity(chars.len() as usize);
    let mut boxes = Vec::with_capacity(chars.len() as usize * 4);
    for ch in chars.iter() {
        codes.push(ch.unicode_value());
        match ch.loose_bounds() {
            Ok(r) if r.width().value > 0.0 || r.height().value > 0.0 => {
                boxes.extend_from_slice(&geom.rect(&r))
            }
            _ => boxes.extend_from_slice(&[0.0; 4]),
        }
    }
    Ok(PageText { codes, boxes })
}

fn links(doc: &PdfDocument, index: u16) -> Result<Vec<Link>, String> {
    let page = doc.pages().get(index).map_err(err)?;
    let geom = Geom::of(&page);
    let mut out = Vec::new();
    for link in page.links().iter() {
        let Ok(rect) = link.rect() else { continue };
        let mut target = Link { rect: geom.rect(&rect), page: None, uri: None };
        if let Some(dest) = link.destination() {
            target.page = dest.page_index().ok();
        } else if let Some(action) = link.action() {
            match &action {
                PdfAction::LocalDestination(a) => {
                    target.page = a.destination().ok().and_then(|d| d.page_index().ok())
                }
                PdfAction::Uri(a) => target.uri = a.uri().ok(),
                _ => {}
            }
        }
        if target.page.is_some() || target.uri.is_some() {
            out.push(target);
        }
    }
    Ok(out)
}

fn bookmark_page(b: &PdfBookmark) -> Option<u16> {
    if let Some(dest) = b.destination() {
        return dest.page_index().ok();
    }
    match b.action()? {
        PdfAction::LocalDestination(a) => a.destination().ok()?.page_index().ok(),
        _ => None,
    }
}

fn outline_level(first: Option<PdfBookmark>, budget: &mut usize, depth: usize) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut node = first;
    // Depth and item limits guard against malformed (cyclic) outlines.
    while let Some(b) = node {
        if *budget == 0 || depth > 32 {
            break;
        }
        *budget -= 1;
        items.push(OutlineItem {
            title: b.title().unwrap_or_default(),
            page: bookmark_page(&b),
            children: outline_level(b.first_child(), budget, depth + 1),
        });
        node = b.next_sibling();
    }
    items
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

        e.close(doc.id).await;
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

    #[tokio::test]
    async fn text_positions_are_top_left() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let t = e.page_text(doc.id, 0).await.unwrap();
        let s: String = t.codes.iter().filter_map(|&c| char::from_u32(c)).collect();
        assert!(s.starts_with("Page 1"), "got {:?}", &s[..20.min(s.len())]);
        // "Page 1" is drawn 100pt below the top edge in 28pt Helvetica-Bold.
        let (top, bottom) = (t.boxes[1], t.boxes[3]);
        assert!(top > 60.0 && top < 100.0 && bottom > 95.0 && bottom < 115.0, "{top}..{bottom}");
        // Landscape page 4 keeps text within its (wider) bounds.
        let t4 = e.page_text(doc.id, 3).await.unwrap();
        assert!(t4.boxes.chunks(4).all(|b| b[2] <= doc.pages[3].width + 1.0));
    }

    #[tokio::test]
    async fn search_finds_hits_case_insensitively() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let hits = e.search(doc.id, "LAZY DOG".into(), false).await.unwrap();
        assert_eq!(hits.len(), 200 * 30);
        assert!(e.search(doc.id, "LAZY DOG".into(), true).await.unwrap().is_empty());
        let t = e.page_text(doc.id, hits[0].page).await.unwrap();
        let found: String = t.codes[hits[0].start as usize..][..hits[0].len as usize]
            .iter()
            .filter_map(|&c| char::from_u32(c))
            .collect();
        assert_eq!(found, "lazy dog");
    }

    #[tokio::test]
    async fn outline_and_links() {
        let Some(path) = corpus("outline-links.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let outline = e.outline(doc.id).await.unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "Chapter 1");
        assert_eq!(outline[1].page, Some(2));
        assert_eq!(outline[0].children[0].title, "Section 1.1");
        let links = e.links(doc.id, 0).await.unwrap();
        assert!(links.iter().any(|l| l.page == Some(2)));
        assert!(links.iter().any(|l| l.uri.as_deref() == Some("https://example.com/")));
    }
}
