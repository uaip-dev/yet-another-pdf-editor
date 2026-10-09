//! PDF engine: owns PDFium on a single dedicated thread.
//!
//! PDFium is not thread-safe, so every operation is a closure sent to the
//! worker thread, which runs it against the open documents and replies over a
//! oneshot channel.
//!
//! All coordinates handed to the UI are in PDF points with a top-left origin,
//! in the page's displayed orientation (after /Rotate and the crop box).

use crate::{annots, docops, edit, forms, pages};
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
pub(crate) struct Geom {
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    rot: u8, // quarter turns clockwise
}

impl Geom {
    pub(crate) fn of(page: &PdfPage) -> Self {
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

    pub(crate) fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let (u, v) = (x - self.left, self.top - y);
        match self.rot {
            1 => (self.h - v, u),
            2 => (self.w - u, self.h - v),
            3 => (v, self.w - u),
            _ => (u, v),
        }
    }

    /// Inverse of `point`: displayed page space to user space.
    pub(crate) fn unpoint(&self, px: f32, py: f32) -> (f32, f32) {
        let (u, v) = match self.rot {
            1 => (py, self.h - px),
            2 => (self.w - px, self.h - py),
            3 => (self.w - py, px),
            _ => (px, py),
        };
        (u + self.left, self.top - v)
    }

    /// Displayed page width and height.
    pub(crate) fn display_size(&self) -> (f32, f32) {
        if self.rot % 2 == 1 {
            (self.h, self.w)
        } else {
            (self.w, self.h)
        }
    }

    /// Returns [left, top, right, bottom] in page space.
    fn rect(&self, r: &PdfRect) -> [f32; 4] {
        self.user_rect([r.left().value, r.bottom().value, r.right().value, r.top().value])
    }

    /// User-space [left, bottom, right, top] to displayed [left, top, right, bottom].
    pub(crate) fn user_rect(&self, r: [f32; 4]) -> [f32; 4] {
        let (ax, ay) = self.point(r[0], r[3]);
        let (bx, by) = self.point(r[2], r[1]);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }

    /// Displayed [left, top, right, bottom] to user-space [left, bottom, right, top].
    pub(crate) fn display_rect(&self, r: [f32; 4]) -> [f32; 4] {
        let (ax, ay) = self.unpoint(r[0], r[1]);
        let (bx, by) = self.unpoint(r[2], r[3]);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }
}

struct OpenDoc<'a> {
    doc: PdfDocument<'a>,
    path: String,
    password: Option<String>,
    /// Encryption to apply when saving (the open document is kept decrypted).
    protection: Option<docops::Protection>,
    text: HashMap<u16, Arc<PageText>>,
    models: HashMap<u16, Arc<edit::PageModel>>,
    fonts: edit::Fonts,
    /// Snapshots of the whole document taken before each edit.
    undo: Vec<Vec<u8>>,
    redo: Vec<Vec<u8>>,
    dirty: bool,
    /// Bumped on every change; edit requests must name the revision they saw.
    revision: u32,
}

/// Result of a change, so the UI can refresh and update its controls.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocState {
    pub revision: u32,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Characters drawn with a substitute font by the last edit.
    pub substituted: String,
    pub path: String,
    /// The page list, when the change may have altered it (page operations, undo/redo).
    pub pages: Option<Vec<PageInfo>>,
}

/// Annotations and form fields of one page.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageExtras {
    pub revision: u32,
    pub annotations: Vec<annots::AnnotInfo>,
    pub fields: Vec<forms::FieldInfo>,
}

/// A change to an existing annotation.
#[derive(serde::Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum AnnotChange {
    Rect([f32; 4]),
    Contents(String),
    Delete,
}

/// Editable targets on a page.
#[derive(Clone, Copy, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    Block,
    Image,
}

const UNDO_LIMIT_BYTES: usize = 256 * 1024 * 1024;
const UNDO_LIMIT_STEPS: usize = 50;
pub const ERR_STALE: &str = "STALE";

impl<'a> OpenDoc<'a> {
    fn state(&self, substituted: String) -> DocState {
        DocState {
            revision: self.revision,
            dirty: self.dirty,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            substituted,
            path: self.path.clone(),
            pages: None,
        }
    }

    /// State including the current page list.
    fn state_with_pages(&self) -> DocState {
        let mut s = self.state(String::new());
        s.pages = Some(page_infos(&self.doc));
        s
    }

    /// Runs a page operation with undo and returns the new page list.
    fn page_op(
        &mut self,
        pdfium: &'a Pdfium,
        op: impl FnOnce(&mut PdfDocument<'a>) -> Result<(), String>,
    ) -> Result<DocState, String> {
        self.snapshot()?;
        if let Err(e) = op(&mut self.doc) {
            // Roll back a half-applied operation to the snapshot just taken.
            if let Some(bytes) = self.undo.pop() {
                let dirty = self.dirty;
                let _ = self.reload(pdfium, bytes);
                self.dirty = dirty;
            }
            return Err(e);
        }
        self.changed();
        Ok(self.state_with_pages())
    }

    fn model(&mut self, page: u16) -> Result<Arc<edit::PageModel>, String> {
        if let Some(m) = self.models.get(&page) {
            return Ok(m.clone());
        }
        let p = self.doc.pages().get(page).map_err(err)?;
        let m = Arc::new(edit::analyse(&p));
        self.models.insert(page, m.clone());
        Ok(m)
    }

    fn check(&self, revision: u32) -> Result<(), String> {
        if revision == self.revision {
            Ok(())
        } else {
            Err(ERR_STALE.into())
        }
    }

    /// Records an undo snapshot; call before changing the document.
    fn snapshot(&mut self) -> Result<(), String> {
        let bytes = self.doc.save_to_bytes().map_err(err)?;
        self.push_undo(bytes);
        Ok(())
    }

    /// Records already-serialized document bytes as the undo point.
    fn push_undo(&mut self, bytes: Vec<u8>) {
        self.undo.push(bytes);
        self.redo.clear();
        while self.undo.len() > UNDO_LIMIT_STEPS
            || (self.undo.len() > 1 && self.undo.iter().map(Vec::len).sum::<usize>() > UNDO_LIMIT_BYTES)
        {
            self.undo.remove(0);
        }
    }

    /// Call after a change: drops caches and bumps the revision.
    fn changed(&mut self) {
        self.text.clear();
        self.models.clear();
        self.dirty = true;
        self.revision += 1;
    }

    /// Replaces the document with one loaded from `bytes` (undo/redo).
    fn reload(&mut self, pdfium: &'a Pdfium, bytes: Vec<u8>) -> Result<(), String> {
        let doc = pdfium
            .load_pdf_from_byte_vec(bytes.clone(), None)
            .or_else(|_| pdfium.load_pdf_from_byte_vec(bytes, self.password.as_deref()))
            .map_err(err)?;
        self.doc = doc;
        self.fonts.loaded.clear(); // tokens belong to the old document
        self.changed();
        Ok(())
    }

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
        self.run(move |docs| {
            // Read into memory so the file is never locked and can be saved over.
            let bytes = std::fs::read(Path::new(&path)).map_err(|e| format!("{e}"))?;
            let mut doc = docs.pdfium.load_pdf_from_byte_vec(bytes.clone(), password.as_deref()).map_err(err)?;
            // Password-protected: work on a decrypted copy so every feature works,
            // and encrypt again on save. If lopdf can't decrypt it, keep PDFium's view.
            let mut protection = None;
            let encrypted = !matches!(doc.permissions().security_handler_revision(), Ok(PdfSecurityHandlerRevision::Unprotected));
            if encrypted {
                match docops::decrypt(&bytes, password.as_deref().unwrap_or("")) {
                    Ok((plain, p)) => match docs.pdfium.load_pdf_from_byte_vec(plain, None) {
                        Ok(d) => {
                            doc = d;
                            protection = Some(p);
                        }
                        Err(e) => log::warn!("decrypted copy did not load: {e:?}"),
                    },
                    Err(e) => log::warn!("could not decrypt for editing: {e}"),
                }
            }
            let id = docs.next_id;
            docs.next_id += 1;
            let info = doc_info(id, &path, &doc);
            docs.open.insert(
                id,
                OpenDoc {
                    doc,
                    path,
                    password,
                    protection,
                    text: HashMap::new(),
                    models: HashMap::new(),
                    fonts: edit::Fonts::default(),
                    undo: Vec::new(),
                    redo: Vec::new(),
                    dirty: false,
                    revision: 0,
                },
            );
            Ok(info)
        })
        .await
    }

    pub async fn state(&self, id: DocId) -> Result<DocState, String> {
        self.run(move |docs| Ok(docs.get(id)?.state(String::new()))).await
    }

    pub async fn page_layout(&self, id: DocId, page: u16) -> Result<(u32, edit::PageLayout), String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            let model = d.model(page)?;
            let geom = Geom::of(&d.doc.pages().get(page).map_err(err)?);
            Ok((d.revision, model.layout(&geom)))
        })
        .await
    }

    pub async fn edit_text(
        &self,
        id: DocId,
        page: u16,
        revision: u32,
        block: usize,
        text: String,
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            d.check(revision)?;
            let model = d.model(page)?;
            let b = model.blocks.get(block).ok_or("Unknown text block")?;
            let current: String = b.chars.iter().map(|c| c.0).collect();
            if current == text.replace("\r\n", "\n") {
                return Ok(d.state(String::new()));
            }
            d.snapshot()?;
            let note = edit::edit_text(&mut d.doc, &mut d.fonts, page, &model, block, &text)?;
            d.changed();
            Ok(d.state(note.substituted))
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_text(
        &self,
        id: DocId,
        page: u16,
        at: (f32, f32),
        text: String,
        size: f32,
        family: String,
        bold: bool,
        italic: bool,
        color: (u8, u8, u8),
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            if text.trim().is_empty() {
                return Ok(d.state(String::new()));
            }
            let geom = Geom::of(&d.doc.pages().get(page).map_err(err)?);
            d.snapshot()?;
            let note = edit::add_text(
                &mut d.doc, &mut d.fonts, page, &geom, at, &text, size, &family, bold, italic, color,
            )?;
            d.changed();
            Ok(d.state(note.substituted))
        })
        .await
    }

    /// Moves/resizes a block or image to a new displayed rect.
    pub async fn transform(
        &self,
        id: DocId,
        page: u16,
        revision: u32,
        target: Target,
        index: usize,
        rect: [f32; 4],
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            d.check(revision)?;
            let model = d.model(page)?;
            let geom = Geom::of(&d.doc.pages().get(page).map_err(err)?);
            let (objects, from) = target_objects(&model, target, index)?;
            d.snapshot()?;
            edit::transform_objects(&d.doc, page, &objects, from, geom.display_rect(rect))?;
            d.changed();
            Ok(d.state(String::new()))
        })
        .await
    }

    pub async fn delete(&self, id: DocId, page: u16, revision: u32, target: Target, index: usize) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            d.check(revision)?;
            let model = d.model(page)?;
            let (objects, _) = target_objects(&model, target, index)?;
            d.snapshot()?;
            edit::delete_objects(&d.doc, page, &objects)?;
            d.changed();
            Ok(d.state(String::new()))
        })
        .await
    }

    pub async fn replace_image(
        &self,
        id: DocId,
        page: u16,
        revision: u32,
        index: usize,
        path: String,
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            d.check(revision)?;
            let model = d.model(page)?;
            let obj = model.images.get(index).ok_or("Unknown image")?.index;
            d.snapshot()?;
            edit::replace_image(&d.doc, page, obj, &path)?;
            d.changed();
            Ok(d.state(String::new()))
        })
        .await
    }

    pub async fn add_image(
        &self,
        id: DocId,
        page: u16,
        at: (f32, f32),
        src: edit::ImageSource,
        width: Option<f32>,
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            let geom = Geom::of(&d.doc.pages().get(page).map_err(err)?);
            d.snapshot()?;
            edit::add_image(&d.doc, page, &geom, at, &src, width)?;
            d.changed();
            Ok(d.state(String::new()))
        })
        .await
    }

    /// Annotations and form fields of a page.
    pub async fn page_extras(&self, id: DocId, page: u16) -> Result<PageExtras, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            let p = d.doc.pages().get(page).map_err(err)?;
            let geom = Geom::of(&p);
            Ok(PageExtras {
                revision: d.revision,
                annotations: annots::list(&p, &geom),
                fields: forms::list(&p, &geom),
            })
        })
        .await
    }

    pub async fn add_annotation(&self, id: DocId, page: u16, new: annots::NewAnnot) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            let geom = Geom::of(&d.doc.pages().get(page).map_err(err)?);
            let current = d.doc.save_to_bytes().map_err(err)?;
            let updated = annots::create(&current, page, &geom, new)?;
            d.push_undo(current);
            d.reload(pdfium, updated)?;
            Ok(d.state(String::new()))
        })
        .await
    }

    pub async fn change_annotation(
        &self,
        id: DocId,
        page: u16,
        revision: u32,
        annot: usize,
        change: AnnotChange,
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            d.check(revision)?;
            let geom = Geom::of(&d.doc.pages().get(page).map_err(err)?);
            d.snapshot()?;
            match change {
                AnnotChange::Rect(r) => annots::set_rect(&d.doc, page, &geom, annot, r)?,
                AnnotChange::Contents(t) => annots::set_contents(&d.doc, page, annot, &t)?,
                AnnotChange::Delete => annots::delete(&d.doc, page, annot)?,
            }
            d.changed();
            Ok(d.state(String::new()))
        })
        .await
    }

    /// Fills form fields on a page through PDFium's form-fill engine.
    pub async fn fill_fields(
        &self,
        id: DocId,
        page: u16,
        revision: u32,
        changes: Vec<(usize, forms::FieldValue)>,
    ) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            d.check(revision)?;
            let current = d.doc.save_to_bytes().map_err(err)?;
            let filled = forms::fill(d.doc.bindings(), &current, d.password.as_deref(), page, &changes)?;
            d.push_undo(current);
            d.reload(pdfium, filled)?;
            Ok(d.state(String::new()))
        })
        .await
    }

    /// Undo (or redo) the last change by swapping in a document snapshot.
    pub async fn undo(&self, id: DocId, redo: bool) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            let Some(bytes) = (if redo { d.redo.pop() } else { d.undo.pop() }) else {
                return Ok(d.state(String::new()));
            };
            let current = d.doc.save_to_bytes().map_err(err)?;
            if redo {
                d.undo.push(current)
            } else {
                d.redo.push(current)
            }
            d.reload(pdfium, bytes)?;
            Ok(d.state_with_pages())
        })
        .await
    }

    pub async fn rotate_pages(&self, id: DocId, list: Vec<u16>, delta: i32) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            docs.get(id)?.page_op(pdfium, |doc| pages::rotate(doc, &list, delta))
        })
        .await
    }

    pub async fn delete_pages(&self, id: DocId, list: Vec<u16>) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            docs.get(id)?.page_op(pdfium, |doc| pages::delete(doc, &list))
        })
        .await
    }

    pub async fn insert_blank_page(&self, id: DocId, at: u16, width: f32, height: f32) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            docs.get(id)?.page_op(pdfium, |doc| pages::insert_blank(doc, at, width, height))
        })
        .await
    }

    pub async fn insert_pages_from_file(&self, id: DocId, at: u16, path: String, password: Option<String>) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            docs.get(id)?.page_op(pdfium, |doc| pages::insert_file(pdfium, doc, at, &path, password.as_deref()).map(|_| ()))
        })
        .await
    }

    pub async fn insert_image_page(&self, id: DocId, at: u16, path: String) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            docs.get(id)?.page_op(pdfium, |doc| pages::insert_image_page(doc, at, &path))
        })
        .await
    }

    /// Moves pages so they sit before original index `before`.
    pub async fn move_pages(&self, id: DocId, list: Vec<u16>, before: u16) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            let current = d.doc.save_to_bytes().map_err(err)?;
            let moved = pages::move_pages(&current, &list, before)?;
            d.push_undo(current);
            d.reload(pdfium, moved)?;
            Ok(d.state_with_pages())
        })
        .await
    }

    pub async fn extract_pages(&self, id: DocId, list: Vec<u16>, out: String) -> Result<(), String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            pages::extract(pdfium, &docs.get(id)?.doc, &list, Path::new(&out))
        })
        .await
    }

    pub async fn split(&self, id: DocId, every: u16, dir: String) -> Result<Vec<String>, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            let stem = Path::new(&d.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
            pages::split(pdfium, &d.doc, every, Path::new(&dir), &stem)
        })
        .await
    }

    pub async fn properties(&self, id: DocId) -> Result<docops::Properties, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            Ok(docops::properties(&d.doc, &d.path, d.protection.as_ref().is_some_and(|p| !p.user_password.is_empty())))
        })
        .await
    }

    pub async fn set_properties(&self, id: DocId, props: docops::Properties) -> Result<DocState, String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            let current = d.doc.save_to_bytes().map_err(err)?;
            let updated = docops::set_properties(&current, &props)?;
            d.push_undo(current);
            d.reload(pdfium, updated)?;
            Ok(d.state(String::new()))
        })
        .await
    }

    pub async fn protection(&self, id: DocId) -> Result<Option<docops::Protection>, String> {
        self.run(move |docs| Ok(docs.get(id)?.protection.clone())).await
    }

    /// Sets (or with None removes) the password protection applied on save.
    pub async fn set_protection(&self, id: DocId, protection: Option<docops::Protection>) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            d.protection = protection;
            d.dirty = true;
            Ok(d.state(String::new()))
        })
        .await
    }

    pub async fn export_images(
        &self,
        id: DocId,
        list: Vec<u16>,
        dpi: f32,
        format: docops::ImageFormat,
        dir: String,
    ) -> Result<Vec<String>, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            let stem = Path::new(&d.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "page".into());
            docops::export_images(&d.doc, &list, dpi, format, Path::new(&dir), &stem)
        })
        .await
    }

    /// Downsamples large images and repacks the file; undoable.
    pub async fn compress(&self, id: DocId, max_dpi: f32, quality: u8) -> Result<(DocState, docops::CompressReport), String> {
        self.run(move |docs| {
            let pdfium = docs.pdfium;
            let d = docs.get(id)?;
            let before = d.doc.save_to_bytes().map_err(err)?;
            let images = docops::downsample_images(&d.doc, max_dpi, quality)?;
            let resampled = d.doc.save_to_bytes().map_err(err)?;
            let packed = docops::repack(&resampled)?;
            let report = docops::CompressReport { before: before.len() as u64, after: packed.len() as u64, images_resampled: images };
            d.push_undo(before);
            d.reload(pdfium, packed)?;
            Ok((d.state_with_pages(), report))
        })
        .await
    }

    /// Saves to `path` (or the document's own path). Writes a temp file and
    /// renames it over the target so a failed save never corrupts the file.
    /// The existing file is first copied to `backup_dir`.
    pub async fn save(&self, id: DocId, path: Option<String>, backup_dir: Option<PathBuf>) -> Result<DocState, String> {
        self.run(move |docs| {
            let d = docs.get(id)?;
            let target = path.unwrap_or_else(|| d.path.clone());
            let mut bytes = d.doc.save_to_bytes().map_err(err)?;
            if let Some(p) = &d.protection {
                bytes = docops::encrypt(&bytes, p)?;
            }
            let target_path = Path::new(&target);
            if let (Some(dir), true) = (backup_dir, target_path.exists()) {
                backup(&dir, target_path);
            }
            let tmp = target_path.with_extension("pdf.yape-tmp");
            std::fs::write(&tmp, &bytes).map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, target_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("Could not save {target}: {e}")
            })?;
            d.path = target;
            d.dirty = false;
            Ok(d.state(String::new()))
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

fn target_objects(model: &edit::PageModel, target: Target, index: usize) -> Result<(Vec<usize>, [f32; 4]), String> {
    match target {
        Target::Block => {
            let b = model.blocks.get(index).ok_or("Unknown text block")?;
            Ok((b.objects.clone(), b.bbox))
        }
        Target::Image => {
            let im = model.images.get(index).ok_or("Unknown image")?;
            Ok((vec![im.index], im.bbox))
        }
    }
}

/// Keeps a copy of a file before it is overwritten (newest 20 kept).
fn backup(dir: &Path, file: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let _ = std::fs::copy(file, dir.join(format!("{stamp}-{name}")));
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        let excess = files.len().saturating_sub(20);
        for f in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(f);
        }
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

pub(crate) fn err(e: PdfiumError) -> String {
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

fn page_infos(doc: &PdfDocument) -> Vec<PageInfo> {
    doc.pages().iter().map(|p| PageInfo { width: p.width().value, height: p.height().value }).collect()
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
    let mut codes = Vec::with_capacity(chars.len());
    let mut boxes = Vec::with_capacity(chars.len() * 4);
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

    fn page_string(t: &PageText) -> String {
        t.codes.iter().filter_map(|&c| char::from_u32(c)).collect()
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("yape-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[tokio::test]
    async fn edit_text_undo_redo_save() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let heading = layout.blocks.iter().find(|b| b.text == "Page 1").expect("heading block");
        assert!(heading.size > 27.0 && heading.size < 29.0);
        assert!(heading.bold && heading.family == "sans-serif");
        // The 30 body lines share font, size and spacing, so they form one paragraph.
        assert!(layout.blocks.iter().any(|b| b.text.starts_with("Line 1: The quick") && b.text.contains("Line 30:")));

        let st = e.edit_text(doc.id, 0, rev, heading.id, "Halaman Satu é".into()).await.unwrap();
        assert!(st.dirty && st.can_undo && !st.can_redo);
        assert_eq!(st.substituted, "", "non-embedded Helvetica covers WinAnsi");
        let text = page_string(&e.page_text(doc.id, 0).await.unwrap());
        assert!(text.contains("Halaman Satu é") && !text.contains("Page 1"), "{}", &text[..60]);

        // A stale revision is rejected.
        assert_eq!(e.edit_text(doc.id, 0, rev, 0, "x".into()).await.err().as_deref(), Some(ERR_STALE));

        let st = e.undo(doc.id, false).await.unwrap();
        assert!(st.can_redo);
        assert!(page_string(&e.page_text(doc.id, 0).await.unwrap()).contains("Page 1"));
        e.undo(doc.id, true).await.unwrap();
        assert!(page_string(&e.page_text(doc.id, 0).await.unwrap()).contains("Halaman Satu"));

        let out = temp_path("edited.pdf");
        let st = e.save(doc.id, Some(out.to_string_lossy().into()), None).await.unwrap();
        assert!(!st.dirty);
        let reopened = e.open(out.to_string_lossy().into(), None).await.unwrap();
        assert_eq!(reopened.pages.len(), 200);
        assert!(page_string(&e.page_text(reopened.id, 0).await.unwrap()).contains("Halaman Satu é"));
    }

    #[tokio::test]
    async fn rewraps_long_paragraph_inside_its_width() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let body = layout.blocks.iter().find(|b| b.text.starts_with("Line 1:")).unwrap();
        let longer = format!("{} and then some more words to force wrapping", body.text);
        e.edit_text(doc.id, 0, rev, body.id, longer).await.unwrap();
        let (_, after) = e.page_layout(doc.id, 0).await.unwrap();
        let b = after.blocks.iter().find(|b| b.text.starts_with("Line 1:")).unwrap();
        assert!(b.text.ends_with("force wrapping"), "{}", b.text);
        assert!(b.rect[2] <= body.rect[2] + body.size, "stays within original width");
    }

    #[tokio::test]
    async fn move_delete_and_add_text() {
        let Some(path) = corpus("image-page.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let cap = layout.blocks.iter().find(|b| b.text.starts_with("Caption")).unwrap();
        let r = cap.rect;
        let moved = [r[0] + 100.0, r[1] + 50.0, r[2] + 100.0, r[3] + 50.0];
        let st = e.transform(doc.id, 0, rev, Target::Block, cap.id, moved).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let cap2 = layout.blocks.iter().find(|b| b.text.starts_with("Caption")).unwrap();
        assert!((cap2.rect[0] - moved[0]).abs() < 0.5 && (cap2.rect[1] - moved[1]).abs() < 0.5);
        assert_eq!(st.revision, rev);

        e.delete(doc.id, 0, rev, Target::Block, cap2.id).await.unwrap();
        assert!(!page_string(&e.page_text(doc.id, 0).await.unwrap()).contains("Caption"));

        e.add_text(doc.id, 0, (100.0, 100.0), "New note\nsecond line".into(), 12.0, "serif".into(), false, true, (200, 0, 0))
            .await
            .unwrap();
        let (_, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let note = layout.blocks.iter().find(|b| b.text.starts_with("New note")).expect("new block");
        assert_eq!(note.text, "New note second line");
        assert!((note.rect[0] - 100.0).abs() < 2.0 && (note.rect[1] - 100.0).abs() < 6.0, "{:?}", note.rect);
        assert!(note.italic && note.family == "serif" && note.color == "#c80000");
    }

    #[tokio::test]
    async fn replace_move_and_add_images() {
        let (Some(path), Some(red), Some(jpg)) = (corpus("image-page.pdf"), corpus("red-square.png"), corpus("green.jpg")) else {
            return;
        };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        assert_eq!(layout.images.len(), 1);
        let im = &layout.images[0];
        assert_eq!((im.width, im.height), (400, 200));

        // A square into a 2:1 frame: fitted to the frame's height and centred.
        e.replace_image(doc.id, 0, rev, 0, red).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let new = &layout.images[0];
        assert_eq!((new.width, new.height), (300, 300));
        let (w, h) = (new.rect[2] - new.rect[0], new.rect[3] - new.rect[1]);
        assert!((w - h).abs() < 0.5 && (h - (im.rect[3] - im.rect[1])).abs() < 0.5, "{w}x{h}");
        assert!(((new.rect[0] + new.rect[2]) / 2.0 - (im.rect[0] + im.rect[2]) / 2.0).abs() < 0.5);

        let target = [50.0, 60.0, 250.0, 260.0];
        e.transform(doc.id, 0, rev, Target::Image, 0, target).await.unwrap();
        let (_, layout) = e.page_layout(doc.id, 0).await.unwrap();
        for (a, b) in layout.images[0].rect.iter().zip(target) {
            assert!((a - b).abs() < 0.5, "{:?}", layout.images[0].rect);
        }

        e.add_image(doc.id, 0, (300.0, 400.0), edit::ImageSource::Path(jpg), None).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        assert_eq!(layout.images.len(), 2);
        let added = layout.images.iter().find(|i| i.width == 160).unwrap();
        assert!((added.rect[0] - 300.0).abs() < 0.5 && (added.rect[1] - 400.0).abs() < 0.5);
        assert!((added.rect[2] - added.rect[0] - 120.0).abs() < 0.5, "160px at 96dpi = 120pt");

        e.delete(doc.id, 0, rev, Target::Image, added.id).await.unwrap();
        assert_eq!(e.page_layout(doc.id, 0).await.unwrap().1.images.len(), 1);
    }

    /// Optional: a real Chromium-generated PDF with subset fonts, if present locally.
    /// Asserts only on structure, never on the (private) content.
    #[tokio::test]
    async fn sample_subset_font_editing() {
        let Some(path) = corpus("Catatan_Mesyuarat_SPP.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let (rev, layout) = e.page_layout(doc.id, 0).await.unwrap();
        assert!(layout.blocks.len() > 5, "glyph objects are grouped into blocks");
        let b = layout.blocks.iter().max_by_key(|b| b.text.len()).unwrap();
        let head: String = b.text.chars().take(5).collect();
        // Append a character no subset here contains, plus covered text.
        let new_text = format!("{} Ω{head}", b.text);
        let st = e.edit_text(doc.id, 0, rev, b.id, new_text.clone()).await.unwrap();
        assert!(st.substituted.contains('Ω'), "substituted: {:?}", st.substituted);
        let text = page_string(&e.page_text(doc.id, 0).await.unwrap());
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let tail: Vec<&str> = new_text.split_whitespace().collect();
        let tail = tail[tail.len().saturating_sub(3)..].join(" ");
        assert!(squash(&text).contains(&tail), "edited text is extractable: {tail:?}");
        let out = temp_path("sample-edited.pdf");
        e.save(doc.id, Some(out.to_string_lossy().into()), None).await.unwrap();
    }

    fn pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 3] {
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let i = 8 + ((y * w + x) * 4) as usize;
        [bytes[i], bytes[i + 1], bytes[i + 2]]
    }

    #[tokio::test]
    async fn annotations_create_list_change_delete() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        // "Line 1: ..." is the first body line: find its rect from the text.
        let t = e.page_text(doc.id, 0).await.unwrap();
        let s = page_string(&t);
        let start = s.find("Line 1:").unwrap();
        let b = &t.boxes[start * 4..start * 4 + 4];
        let line = [b[0], b[1], b[0] + 200.0, b[3]];

        use annots::{MarkupKind, NewAnnot, ShapeKind};
        let yellow = [255, 210, 0];
        e.add_annotation(doc.id, 0, NewAnnot::Markup { kind: MarkupKind::Highlight, rects: vec![line], color: yellow }).await.unwrap();
        let under = [line[0], line[1] + 18.0, line[2], line[3] + 18.0];
        e.add_annotation(doc.id, 0, NewAnnot::Markup { kind: MarkupKind::Underline, rects: vec![under], color: [0, 0, 255] }).await.unwrap();
        e.add_annotation(doc.id, 0, NewAnnot::Note { at: (500.0, 120.0), text: "Check this".into(), color: yellow }).await.unwrap();
        let stroke = vec![(100.0, 700.0), (150.0, 720.0), (200.0, 700.0)];
        e.add_annotation(doc.id, 0, NewAnnot::Ink { strokes: vec![stroke], color: [220, 0, 0], width: 3.0 }).await.unwrap();
        e.add_annotation(doc.id, 0, NewAnnot::Shape { kind: ShapeKind::Rectangle, rect: [300.0, 650.0, 400.0, 720.0], color: [0, 128, 0], width: 2.0 })
            .await
            .unwrap();
        e.add_annotation(doc.id, 0, NewAnnot::Shape { kind: ShapeKind::Ellipse, rect: [420.0, 650.0, 520.0, 720.0], color: [0, 0, 200], width: 2.0 })
            .await
            .unwrap();

        let x = e.page_extras(doc.id, 0).await.unwrap();
        let kinds: Vec<&str> = x.annotations.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, ["highlight", "underline", "note", "ink", "square", "circle"]);
        let note = &x.annotations[2];
        assert_eq!(note.contents, "Check this");
        assert!((note.rect[0] - 500.0).abs() < 0.5 && (note.rect[1] - 120.0).abs() < 0.5);
        assert!(!x.annotations[0].movable && note.movable);

        // The highlight is drawn: its area is yellowish when rendered at 1px/pt.
        let img = e.render(doc.id, 0, 595).await.unwrap();
        let [r, g, bl] = pixel(&img, (line[0] + 3.0) as u32, ((line[1] + line[3]) / 2.0) as u32);
        assert!(r > 200 && g > 170 && bl < 120, "highlight colour {r},{g},{bl}");
        // The red pen stroke is drawn too.
        let [r, g, _] = pixel(&img, 150, 719);
        assert!(r > 150 && g < 100, "ink colour {r},{g}");

        // Move the note, change its text, delete the ellipse.
        let st = e.change_annotation(doc.id, 0, x.revision, note.id, AnnotChange::Rect([50.0, 50.0, 70.0, 70.0])).await.unwrap();
        let st = e.change_annotation(doc.id, 0, st.revision, note.id, AnnotChange::Contents("Done".into())).await.unwrap();
        e.change_annotation(doc.id, 0, st.revision, x.annotations[5].id, AnnotChange::Delete).await.unwrap();
        let x = e.page_extras(doc.id, 0).await.unwrap();
        assert_eq!(x.annotations.len(), 5);
        let note = x.annotations.iter().find(|a| a.kind == "note").unwrap();
        assert_eq!(note.contents, "Done");
        assert!((note.rect[0] - 50.0).abs() < 0.5 && (note.rect[1] - 50.0).abs() < 0.5);

        // Survives save + reopen.
        let out = temp_path("annotated.pdf");
        e.save(doc.id, Some(out.to_string_lossy().into()), None).await.unwrap();
        let re = e.open(out.to_string_lossy().into(), None).await.unwrap();
        assert_eq!(e.page_extras(re.id, 0).await.unwrap().annotations.len(), 5);
    }

    #[tokio::test]
    async fn fill_form_fields() {
        let Some(path) = corpus("form.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let x = e.page_extras(doc.id, 0).await.unwrap();
        let by = |name: &str| x.fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("field {name}"));
        assert_eq!(by("name").kind, "text");
        assert!(by("notes").multiline);
        assert_eq!(by("agree").kind, "checkbox");
        assert_eq!(by("country").kind, "combo");
        assert_eq!(by("country").options, ["Malaysia", "Singapore", "Indonesia"]);
        let radios: Vec<_> = x.fields.iter().filter(|f| f.kind == "radio").collect();
        assert_eq!(radios.len(), 3);
        assert!(radios.iter().all(|r| !r.checked), "no radio selected initially");

        use forms::FieldValue;
        let changes = vec![
            (by("name").id, FieldValue::Text("Siti Aminah".into())),
            (by("notes").id, FieldValue::Text("Line one\nLine two".into())),
            (by("agree").id, FieldValue::Checked(true)),
            (radios[1].id, FieldValue::Checked(true)),
            (by("country").id, FieldValue::Select(1)),
        ];
        let st = e.fill_fields(doc.id, 0, x.revision, changes).await.unwrap();
        assert!(st.dirty && st.can_undo);

        let x = e.page_extras(doc.id, 0).await.unwrap();
        let by = |name: &str| x.fields.iter().find(|f| f.name == name).unwrap();
        assert_eq!(by("name").value, "Siti Aminah");
        assert!(by("notes").value.contains("Line two"));
        assert!(by("agree").checked);
        let checked: Vec<bool> = x.fields.iter().filter(|f| f.kind == "radio").map(|f| f.checked).collect();
        assert_eq!(checked, [false, true, false]);
        assert_eq!(by("country").value, "Singapore");

        // The value is drawn into the field's appearance (dark pixels inside the box).
        let img = e.render(doc.id, 0, 595).await.unwrap();
        let r = by("name").rect;
        let mut dark = 0;
        for x in (r[0] as u32 + 2)..(r[2] as u32 - 2) {
            for y in (r[1] as u32 + 2)..(r[3] as u32 - 2) {
                if pixel(&img, x, y).iter().all(|&c| c < 100) {
                    dark += 1;
                }
            }
        }
        assert!(dark > 20, "field text rendered ({dark} dark px)");

        // Undo returns to the empty form.
        e.undo(doc.id, false).await.unwrap();
        let x = e.page_extras(doc.id, 0).await.unwrap();
        assert_eq!(x.fields.iter().find(|f| f.name == "name").unwrap().value, "");
    }

    #[tokio::test]
    async fn signature_from_png_bytes() {
        let Some(path) = corpus("letter.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let png = std::fs::read(corpus("red-square.png").unwrap()).unwrap();
        e.add_image(doc.id, 0, (100.0, 500.0), edit::ImageSource::Bytes(png), Some(150.0)).await.unwrap();
        let (_, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let im = &layout.images[0];
        assert!((im.rect[2] - im.rect[0] - 150.0).abs() < 0.5 && (im.rect[3] - im.rect[1] - 150.0).abs() < 0.5);
    }

    async fn first_line(e: &Engine, id: DocId, page: u16) -> String {
        page_string(&e.page_text(id, page).await.unwrap()).lines().next().unwrap_or("").trim().to_string()
    }

    #[tokio::test]
    async fn rotate_delete_move_insert_pages() {
        let (Some(path), Some(letter), Some(png)) = (corpus("multipage.pdf"), corpus("letter.pdf"), corpus("blue.png")) else {
            return;
        };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();

        // Rotate: width and height swap; rotating back restores them.
        let st = e.rotate_pages(doc.id, vec![0], 90).await.unwrap();
        let p = st.pages.unwrap();
        assert_eq!((p[0].width.round(), p[0].height.round()), (842.0, 595.0));
        let p = e.rotate_pages(doc.id, vec![0], -90).await.unwrap().pages.unwrap();
        assert_eq!((p[0].width.round(), p[0].height.round()), (595.0, 842.0));

        // Delete pages 2 and 3.
        let st = e.delete_pages(doc.id, vec![1, 2]).await.unwrap();
        assert_eq!(st.pages.unwrap().len(), 198);
        assert_eq!(first_line(&e, doc.id, 1).await, "Page 4");

        // Undo brings them back (with the page list).
        let st = e.undo(doc.id, false).await.unwrap();
        assert_eq!(st.pages.unwrap().len(), 200);
        assert_eq!(first_line(&e, doc.id, 1).await, "Page 2");

        // Move pages 6 and 7 to the front; landscape page 4 keeps its size.
        let st = e.move_pages(doc.id, vec![5, 6], 0).await.unwrap();
        let p = st.pages.unwrap();
        assert_eq!(p.len(), 200);
        assert_eq!(first_line(&e, doc.id, 0).await, "Page 6");
        assert_eq!(first_line(&e, doc.id, 1).await, "Page 7");
        assert_eq!(first_line(&e, doc.id, 2).await, "Page 1");
        assert!(p[5].width > p[5].height, "page 4 (now index 5) is still landscape");
        // And to the end.
        e.move_pages(doc.id, vec![0], 200).await.unwrap();
        assert_eq!(first_line(&e, doc.id, 199).await, "Page 6");

        // Insert blank, another PDF, and an image page.
        let p = e.insert_blank_page(doc.id, 0, 300.0, 400.0).await.unwrap().pages.unwrap();
        assert_eq!((p.len(), p[0].width, p[0].height), (201, 300.0, 400.0));
        let p = e.insert_pages_from_file(doc.id, 201, letter, None).await.unwrap().pages.unwrap();
        assert_eq!((p.len(), p[201].width, p[201].height), (202, 612.0, 792.0));
        let p = e.insert_image_page(doc.id, 1, png).await.unwrap().pages.unwrap();
        assert_eq!(p.len(), 203);
        assert!((p[1].width - 842.0).abs() < 0.5 && (p[1].height - 421.0).abs() < 0.5, "{:?}", (p[1].width, p[1].height));
        assert_eq!(e.page_layout(doc.id, 1).await.unwrap().1.images.len(), 1);

        // A document must keep at least one page.
        let all: Vec<u16> = (0..203).collect();
        assert!(e.delete_pages(doc.id, all).await.is_err());
        assert_eq!(e.state(doc.id).await.unwrap().can_undo, true);
    }

    #[tokio::test]
    async fn moving_pages_keeps_bookmarks_and_links() {
        let Some(path) = corpus("outline-links.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        // Move the last page (Chapter 2's target) to the front.
        e.move_pages(doc.id, vec![2], 0).await.unwrap();
        let outline = e.outline(doc.id).await.unwrap();
        assert_eq!(outline[1].title, "Chapter 2");
        assert_eq!(outline[1].page, Some(0), "bookmark follows its page");
        assert_eq!(outline[0].page, Some(1));
        // The internal link on the old first page (now index 1) still targets page "3" (now 0).
        let links = e.links(doc.id, 1).await.unwrap();
        assert!(links.iter().any(|l| l.page == Some(0)));
    }

    #[tokio::test]
    async fn extract_and_split() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let out = temp_path("extract.pdf");
        e.extract_pages(doc.id, vec![0, 3], out.to_string_lossy().into()).await.unwrap();
        let x = e.open(out.to_string_lossy().into(), None).await.unwrap();
        assert_eq!(x.pages.len(), 2);
        assert!(x.pages[1].width > x.pages[1].height);
        assert_eq!(first_line(&e, x.id, 1).await, "Page 4");

        let dir = temp_path("split");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = e.split(doc.id, 80, dir.to_string_lossy().into()).await.unwrap();
        let names: Vec<String> = files.iter().map(|f| Path::new(f).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["multipage (1-80).pdf", "multipage (81-160).pdf", "multipage (161-200).pdf"]);
        let last = e.open(files[2].clone(), None).await.unwrap();
        assert_eq!(last.pages.len(), 40);
        assert_eq!(first_line(&e, last.id, 0).await, "Page 161");
    }

    #[tokio::test]
    async fn export_pages_as_images() {
        let Some(path) = corpus("multipage.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let dir = temp_path("export");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = e
            .export_images(doc.id, vec![0, 3], 150.0, docops::ImageFormat::Png, dir.to_string_lossy().into())
            .await
            .unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("multipage - page 1.png"));
        assert_eq!(image::image_dimensions(&files[0]).unwrap().0, 1240, "A4 at 150 dpi");
        let (w, h) = image::image_dimensions(&files[1]).unwrap();
        assert!(w > h, "landscape page 4");
        let jpg = e.export_images(doc.id, vec![1], 72.0, docops::ImageFormat::Jpeg, dir.to_string_lossy().into()).await.unwrap();
        assert!(jpg[0].ends_with(".jpg") && image::image_dimensions(&jpg[0]).unwrap().0 == 595);
    }

    #[tokio::test]
    async fn edit_document_properties() {
        let Some(path) = corpus("letter.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let mut p = e.properties(doc.id).await.unwrap();
        assert_eq!(p.page_count, 1);
        assert!(p.page_size.contains("Letter"), "{}", p.page_size);
        p.title = "Laporan Tahunan".into();
        p.author = "Siti Aminah".into();
        p.keywords = "laporan, 2026".into();
        e.set_properties(doc.id, p).await.unwrap();
        let out = temp_path("props.pdf");
        e.save(doc.id, Some(out.to_string_lossy().into()), None).await.unwrap();
        let re = e.open(out.to_string_lossy().into(), None).await.unwrap();
        assert_eq!(re.title.as_deref(), Some("Laporan Tahunan"));
        let q = e.properties(re.id).await.unwrap();
        assert_eq!((q.author.as_str(), q.keywords.as_str()), ("Siti Aminah", "laporan, 2026"));
    }

    #[tokio::test]
    async fn password_protect_and_edit_protected_documents() {
        let (Some(plain), Some(locked)) = (corpus("letter.pdf"), corpus("encrypted-password-test.pdf")) else { return };
        let e = engine();

        // Protect a plain document; it needs the password afterwards.
        let doc = e.open(plain, None).await.unwrap();
        let prot = docops::Protection { user_password: "rahsia".into(), allow_print: true, ..Default::default() };
        e.set_protection(doc.id, Some(prot)).await.unwrap();
        let out = temp_path("protected.pdf");
        e.save(doc.id, Some(out.to_string_lossy().into()), None).await.unwrap();
        let out = out.to_string_lossy().into_owned();
        assert_eq!(e.open(out.clone(), None).await.err().as_deref(), Some(ERR_PASSWORD));
        assert_eq!(e.open(out.clone(), Some("salah".into())).await.err().as_deref(), Some(ERR_PASSWORD));
        let re = e.open(out.clone(), Some("rahsia".into())).await.unwrap();
        assert!(e.properties(re.id).await.unwrap().protected);

        // A protected document (RC4, from pypdf) can be annotated and stays protected.
        let doc = e.open(locked, Some("test".into())).await.unwrap();
        e.add_annotation(doc.id, 0, annots::NewAnnot::Note { at: (50.0, 50.0), text: "ok".into(), color: [255, 210, 0] })
            .await
            .expect("annotations work on decrypted copy");
        e.move_pages(doc.id, vec![0], 1).await.expect("lopdf features work too");
        let out2 = temp_path("protected-annotated.pdf").to_string_lossy().into_owned();
        e.save(doc.id, Some(out2.clone()), None).await.unwrap();
        assert_eq!(e.open(out2.clone(), None).await.err().as_deref(), Some(ERR_PASSWORD));
        let re = e.open(out2, Some("test".into())).await.unwrap();
        assert_eq!(e.page_extras(re.id, 0).await.unwrap().annotations.len(), 1);

        // Removing the protection saves an open file.
        e.set_protection(re.id, None).await.unwrap();
        let out3 = temp_path("unprotected.pdf").to_string_lossy().into_owned();
        e.save(re.id, Some(out3.clone()), None).await.unwrap();
        assert!(e.open(out3, None).await.is_ok());
    }

    #[tokio::test]
    async fn compress_downsamples_large_images() {
        let Some(path) = corpus("big-image.pdf") else { return };
        let e = engine();
        let doc = e.open(path, None).await.unwrap();
        let (st, report) = e.compress(doc.id, 150.0, 75).await.unwrap();
        assert_eq!(report.images_resampled, 1);
        assert!(report.after * 5 < report.before, "{} -> {}", report.before, report.after);
        assert!(st.dirty && st.can_undo);
        let (_, layout) = e.page_layout(doc.id, 0).await.unwrap();
        let im = &layout.images[0];
        assert!(im.width < 500 && im.width > 300, "~150 dpi for 200pt: {}", im.width);
        assert!((im.rect[2] - im.rect[0] - 200.0).abs() < 0.5, "placement kept");
        assert!(page_string(&e.page_text(doc.id, 0).await.unwrap()).contains("Big image test"));
    }
}
