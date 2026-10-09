//! Page management: rotate, delete, move, insert (blank / from file / image),
//! extract and split.
//!
//! Most operations use PDFium. Moving pages uses lopdf: PDFium only offers
//! moving via its raw API, and rebuilding the document in a new order would
//! drop document-level data (bookmarks, form, metadata). lopdf rewrites the
//! page tree as an incremental update instead, keeping everything else.

use crate::engine::err;
use lopdf::{Object, ObjectId};
use pdfium_render::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn rotation_degrees(r: PdfPageRenderRotation) -> i32 {
    match r {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 90,
        PdfPageRenderRotation::Degrees180 => 180,
        PdfPageRenderRotation::Degrees270 => 270,
    }
}

fn rotation_from(deg: i32) -> PdfPageRenderRotation {
    match deg.rem_euclid(360) {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

/// Rotates pages by a multiple of 90 degrees (clockwise when positive).
pub fn rotate(doc: &PdfDocument, pages: &[u16], delta: i32) -> Result<(), String> {
    for &i in pages {
        let mut page = doc.pages().get(i).map_err(err)?;
        let current = rotation_degrees(page.rotation().unwrap_or(PdfPageRenderRotation::None));
        page.set_rotation(rotation_from(current + delta));
    }
    Ok(())
}

pub fn delete(doc: &PdfDocument, pages: &[u16]) -> Result<(), String> {
    let mut sorted = pages.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() >= doc.pages().len() as usize {
        return Err("A document must keep at least one page.".into());
    }
    for &i in sorted.iter().rev() {
        doc.pages().get(i).map_err(err)?.delete().map_err(err)?;
    }
    Ok(())
}

/// Inserts a blank page of the given size (points) before `at`.
pub fn insert_blank(doc: &mut PdfDocument, at: u16, width: f32, height: f32) -> Result<(), String> {
    let size = PdfPagePaperSize::Custom(PdfPoints::new(width), PdfPoints::new(height));
    doc.pages_mut().create_page_at_index(size, at).map_err(err)?;
    Ok(())
}

/// Inserts all pages of another PDF before `at`.
pub fn insert_file(pdfium: &Pdfium, doc: &mut PdfDocument, at: u16, path: &str, password: Option<&str>) -> Result<u16, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let src = pdfium.load_pdf_from_byte_vec(bytes, password).map_err(err)?;
    let n = src.pages().len();
    if n == 0 {
        return Err("That PDF has no pages.".into());
    }
    doc.pages_mut().copy_page_range_from_document(&src, 0..=n - 1, at).map_err(err)?;
    Ok(n)
}

/// Inserts an image as a new page sized to fit A4 in the image's orientation.
pub fn insert_image_page(doc: &mut PdfDocument, at: u16, path: &str) -> Result<(), String> {
    let (pw, ph) = image::image_dimensions(path).map_err(|e| format!("Cannot read image: {e}"))?;
    let (bw, bh) = if pw > ph { (842.0, 595.0) } else { (595.0, 842.0) };
    let scale = (bw / pw as f32).min(bh / ph as f32);
    let (w, h) = (pw as f32 * scale, ph as f32 * scale);
    insert_blank(doc, at, w, h)?;
    let geom = crate::engine::Geom::of(&doc.pages().get(at).map_err(err)?);
    crate::edit::add_image(doc, at, &geom, (0.0, 0.0), &crate::edit::ImageSource::Path(path.to_string()), Some(w))
}

/// Writes the given pages (in order) to a new PDF file.
pub fn extract(pdfium: &Pdfium, doc: &PdfDocument, pages: &[u16], out: &Path) -> Result<(), String> {
    let mut new = pdfium.create_new_pdf().map_err(err)?;
    let spec = pages.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(",");
    new.pages_mut().copy_pages_from_document(doc, &spec, 0).map_err(err)?;
    write_atomic(&new.save_to_bytes().map_err(err)?, out)
}

/// Splits the document into files of `every` pages: `<stem> (1-5).pdf`, ...
pub fn split(pdfium: &Pdfium, doc: &PdfDocument, every: u16, dir: &Path, stem: &str) -> Result<Vec<String>, String> {
    let n = doc.pages().len();
    let every = every.max(1);
    let mut written = Vec::new();
    let mut start = 0;
    while start < n {
        let end = (start + every).min(n) - 1;
        let label = if start == end { format!("{}", start + 1) } else { format!("{}-{}", start + 1, end + 1) };
        let out = dir.join(format!("{stem} ({label}).pdf"));
        let pages: Vec<u16> = (start..=end).collect();
        extract(pdfium, doc, &pages, &out)?;
        written.push(out.to_string_lossy().into_owned());
        start = end + 1;
    }
    Ok(written)
}

fn write_atomic(bytes: &[u8], out: &Path) -> Result<(), String> {
    let tmp: PathBuf = out.with_extension("pdf.yape-tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not write {}: {e}", out.display())
    })
}

// ---- Moving pages (lopdf) ----

const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn lo(e: lopdf::Error) -> String {
    format!("PDF structure error: {e}")
}

/// Moves `pages` (in their current order) so they sit before original index
/// `before` (use the page count to move to the end). Returns new bytes.
pub fn move_pages(bytes: &[u8], pages: &[u16], before: u16) -> Result<Vec<u8>, String> {
    let prev = lopdf::Document::load_mem(bytes).map_err(lo)?;
    if prev.is_encrypted() {
        return Err("Pages of password-protected PDFs can't be reordered yet.".into());
    }
    let order: Vec<ObjectId> = prev.get_pages().into_values().collect();
    let moving: HashSet<u16> = pages.iter().copied().collect();
    let picked: Vec<ObjectId> = pages.iter().filter_map(|&p| order.get(p as usize).copied()).collect();
    let mut new_order = Vec::with_capacity(order.len());
    for (i, id) in order.iter().enumerate() {
        if i as u16 == before {
            new_order.extend(&picked);
        }
        if !moving.contains(&(i as u16)) {
            new_order.push(*id);
        }
    }
    if before as usize >= order.len() {
        new_order.extend(&picked);
    }
    if new_order == order {
        return Ok(bytes.to_vec());
    }

    let root_pages = prev
        .catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(lo)?;
    // Resolve inherited attributes before flattening the tree.
    let mut inherited: Vec<(ObjectId, Vec<(Vec<u8>, Object)>)> = Vec::new();
    for &id in &order {
        let mut found = Vec::new();
        let page = prev.get_dictionary(id).map_err(lo)?;
        for key in INHERITABLE {
            if page.has(key) {
                continue;
            }
            let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
            while let Some(pid) = parent {
                let Ok(node) = prev.get_dictionary(pid) else { break };
                if let Ok(v) = node.get(key) {
                    found.push((key.to_vec(), v.clone()));
                    break;
                }
                parent = node.get(b"Parent").and_then(Object::as_reference).ok();
            }
        }
        inherited.push((id, found));
    }

    let mut inc = lopdf::IncrementalDocument::create_from(bytes.to_vec(), prev);
    for (id, found) in inherited {
        inc.opt_clone_object_to_new_document(id).map_err(lo)?;
        let page = inc.new_document.get_object_mut(id).and_then(Object::as_dict_mut).map_err(lo)?;
        for (k, v) in found {
            page.set(k, v);
        }
        page.set("Parent", Object::Reference(root_pages));
    }
    inc.opt_clone_object_to_new_document(root_pages).map_err(lo)?;
    let root = inc.new_document.get_object_mut(root_pages).and_then(Object::as_dict_mut).map_err(lo)?;
    root.set("Kids", new_order.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>());
    root.set("Count", new_order.len() as i64);
    for key in INHERITABLE {
        root.remove(key); // now set on each page
    }
    let mut out = Vec::with_capacity(bytes.len() + 4096);
    inc.save_to(&mut out).map_err(|e| format!("Could not write the new page order: {e}"))?;
    Ok(out)
}
