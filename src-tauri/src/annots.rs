//! Annotations: text markup, sticky notes, freehand ink and shapes.
//!
//! New annotations are written with lopdf (see `create`), each with an
//! explicit appearance stream so every viewer draws them the same way.
//! Listing, moving, editing note text and deleting use PDFium.
//!
//! Annotations are separate from page content, so editing them never touches
//! the page's text or images.

use crate::engine::{err, Geom};
use lopdf::{dictionary, Dictionary, Object};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotInfo {
    /// Index among the page's annotations.
    pub id: usize,
    pub kind: &'static str,
    /// left, top, right, bottom in displayed page space
    pub rect: [f32; 4],
    pub color: String,
    pub contents: String,
    pub author: String,
    /// Whether the annotation can be moved (text markup is tied to its text).
    pub movable: bool,
}

/// An RGB colour as sent by the UI.
pub type Rgb = [u8; 3];

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NewAnnot {
    /// Text markup over one rect per line (displayed page space).
    #[serde(rename_all = "camelCase")]
    Markup { kind: MarkupKind, rects: Vec<[f32; 4]>, color: Rgb },
    /// Sticky note with its icon's top-left at `at`.
    #[serde(rename_all = "camelCase")]
    Note { at: (f32, f32), text: String, color: Rgb },
    /// Freehand strokes (displayed page space points).
    #[serde(rename_all = "camelCase")]
    Ink { strokes: Vec<Vec<(f32, f32)>>, color: Rgb, width: f32 },
    /// Rectangle or ellipse inside a displayed rect.
    #[serde(rename_all = "camelCase")]
    Shape { kind: ShapeKind, rect: [f32; 4], color: Rgb, width: f32 },
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum MarkupKind {
    Highlight,
    Underline,
    Strikeout,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
}

fn kind_name(t: PdfPageAnnotationType) -> Option<&'static str> {
    Some(match t {
        PdfPageAnnotationType::Highlight => "highlight",
        PdfPageAnnotationType::Underline => "underline",
        PdfPageAnnotationType::Strikeout => "strikeout",
        PdfPageAnnotationType::Squiggly => "squiggly",
        PdfPageAnnotationType::Text => "note",
        PdfPageAnnotationType::FreeText => "freetext",
        PdfPageAnnotationType::Ink => "ink",
        PdfPageAnnotationType::Square => "square",
        PdfPageAnnotationType::Circle => "circle",
        PdfPageAnnotationType::Line => "line",
        PdfPageAnnotationType::Polygon => "polygon",
        PdfPageAnnotationType::Polyline => "polyline",
        PdfPageAnnotationType::Stamp => "stamp",
        PdfPageAnnotationType::Caret => "caret",
        PdfPageAnnotationType::FileAttachment => "attachment",
        // Links and form widgets have their own UI; popups belong to their parent.
        _ => return None,
    })
}

fn css(c: PdfColor) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

fn rect4(r: &PdfRect) -> [f32; 4] {
    [r.left().value, r.bottom().value, r.right().value, r.top().value]
}

fn pdf_rect(r: [f32; 4]) -> PdfRect {
    // [left, bottom, right, top] -> PdfRect::new(bottom, left, top, right)
    PdfRect::new_from_values(r[1], r[0], r[3], r[2])
}

/// Lists the page's user-visible annotations.
pub fn list(page: &PdfPage, geom: &Geom) -> Vec<AnnotInfo> {
    let mut out = Vec::new();
    for (id, a) in page.annotations().iter().enumerate() {
        let Some(kind) = kind_name(a.annotation_type()) else { continue };
        if a.is_hidden() {
            continue;
        }
        let Ok(b) = a.bounds() else { continue };
        // pdfium-render's annotation colour getters cast the annotation handle
        // to a page object when an appearance stream exists, which crashes
        // PDFium; read ink/stamp colours from their appearance objects instead.
        let color = match &a {
            PdfPageAnnotation::Ink(_) | PdfPageAnnotation::Stamp(_) => a
                .objects()
                .iter()
                .find_map(|o| o.stroke_color().ok().or_else(|| o.fill_color().ok()))
                .map(css)
                .unwrap_or_default(),
            _ => String::new(),
        };
        out.push(AnnotInfo {
            id,
            kind,
            rect: geom.user_rect(rect4(&b)),
            color,
            contents: a.contents().unwrap_or_default(),
            author: a.creator().unwrap_or_default(),
            movable: !matches!(kind, "highlight" | "underline" | "strikeout" | "squiggly"),
        });
    }
    out
}

fn author() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default()
}

fn union(rects: &[[f32; 4]]) -> [f32; 4] {
    rects.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, r| {
        [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]
    })
}

/// Adds an annotation to a document given as bytes and returns the new bytes.
///
/// Written with lopdf as an incremental update: the original bytes are kept
/// as-is and only the new annotation, its appearance stream and the updated
/// page/`Annots` objects are appended. PDFium is not used here because it
/// writes annotations as direct dictionaries (the spec requires indirect
/// references, and some viewers ignore direct ones) and does not generate
/// appearance streams for text markup or notes.
pub fn create(bytes: &[u8], page_index: u16, geom: &Geom, new: NewAnnot) -> Result<Vec<u8>, String> {
    let prev = lopdf::Document::load_mem(bytes).map_err(|e| format!("Could not read the PDF structure: {e}"))?;
    if prev.is_encrypted() {
        return Err("Annotations can't be added to password-protected PDFs yet.".into());
    }
    let page_id = *prev.get_pages().get(&(page_index as u32 + 1)).ok_or("Page not found")?;
    let mut inc = lopdf::IncrementalDocument::create_from(bytes.to_vec(), prev);

    let (mut annot, ap) = build(geom, new)?;
    let ap_id = inc.new_document.add_object(ap);
    annot.set("AP", dictionary! { "N" => Object::Reference(ap_id) });
    annot.set("P", Object::Reference(page_id));
    let annot_id = inc.new_document.add_object(annot);

    // Append to the page's /Annots (direct array, referenced array, or new).
    inc.opt_clone_object_to_new_document(page_id).map_err(lo)?;
    let annots_ref = inc
        .new_document
        .get_object(page_id)
        .and_then(Object::as_dict)
        .ok()
        .and_then(|d| d.get(b"Annots").ok())
        .and_then(|o| o.as_reference().ok());
    match annots_ref {
        Some(arr_id) => {
            inc.opt_clone_object_to_new_document(arr_id).map_err(lo)?;
            let arr = inc.new_document.get_object_mut(arr_id).and_then(Object::as_array_mut).map_err(lo)?;
            arr.push(Object::Reference(annot_id));
        }
        None => {
            let page = inc.new_document.get_object_mut(page_id).and_then(Object::as_dict_mut).map_err(lo)?;
            match page.get_mut(b"Annots").ok().and_then(|o| o.as_array_mut().ok()) {
                Some(arr) => arr.push(Object::Reference(annot_id)),
                None => page.set("Annots", vec![Object::Reference(annot_id)]),
            }
        }
    }
    let mut out = Vec::with_capacity(bytes.len() + 4096);
    inc.save_to(&mut out).map_err(|e| format!("Could not write the annotation: {e}"))?;
    Ok(out)
}

fn lo(e: lopdf::Error) -> String {
    format!("PDF structure error: {e}")
}

fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

fn rgb(c: Rgb) -> String {
    format!("{} {} {}", num(c[0] as f32 / 255.0), num(c[1] as f32 / 255.0), num(c[2] as f32 / 255.0))
}

fn reals(v: &[f32]) -> Object {
    Object::Array(v.iter().map(|&x| Object::Real(x)).collect())
}

/// A PDF text string: PDFDocEncoding-compatible ASCII as-is, otherwise UTF-16BE with BOM.
fn text(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), lopdf::StringFormat::Literal)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, lopdf::StringFormat::Hexadecimal)
    }
}

fn pdf_date() -> Object {
    text(&chrono::Utc::now().format("D:%Y%m%d%H%M%SZ").to_string())
}

/// Builds the annotation dictionary and its appearance stream (both in user space).
fn build(geom: &Geom, new: NewAnnot) -> Result<(Dictionary, lopdf::Stream), String> {
    let mut d = dictionary! {
        "Type" => "Annot",
        "F" => 4, // print
        "CreationDate" => pdf_date(),
        "M" => pdf_date(),
        "NM" => text(&format!("yape-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0))),
    };
    let who = author();
    if !who.is_empty() {
        d.set("T", text(&who));
    }
    let mut resources = Dictionary::new();
    let (bbox, content): ([f32; 4], String) = match new {
        NewAnnot::Markup { kind, rects, color } => {
            if rects.is_empty() {
                return Err("Nothing selected".into());
            }
            let user: Vec<[f32; 4]> = rects.iter().map(|&r| geom.display_rect(r)).collect();
            let bbox = union(&user);
            let mut quads = Vec::new();
            let mut ops = String::new();
            for r in &user {
                let [l, b, rr, t] = *r;
                // Quad order: upper-left, upper-right, lower-left, lower-right.
                quads.extend_from_slice(&[l, t, rr, t, l, b, rr, b]);
                let h = t - b;
                match kind {
                    MarkupKind::Highlight => ops += &format!("{} {} {} {} re f\n", num(l), num(b), num(rr - l), num(h)),
                    MarkupKind::Underline => {
                        let th = (h * 0.07).max(0.6);
                        ops += &format!("{} {} {} {} re f\n", num(l), num(b + h * 0.06), num(rr - l), num(th));
                    }
                    MarkupKind::Strikeout => {
                        let th = (h * 0.07).max(0.6);
                        ops += &format!("{} {} {} {} re f\n", num(l), num(b + h * 0.42), num(rr - l), num(th));
                    }
                }
            }
            let subtype = match kind {
                MarkupKind::Highlight => "Highlight",
                MarkupKind::Underline => "Underline",
                MarkupKind::Strikeout => "StrikeOut",
            };
            d.set("Subtype", Object::Name(subtype.into()));
            d.set("QuadPoints", reals(&quads));
            d.set("C", reals(&color.map(|c| c as f32 / 255.0)));
            let mut content = String::new();
            if let MarkupKind::Highlight = kind {
                // Multiply keeps the text underneath crisp, like a real highlighter.
                resources.set("ExtGState", dictionary! { "GS0" => dictionary! { "Type" => "ExtGState", "BM" => "Multiply" } });
                content += "/GS0 gs\n";
            }
            content += &format!("{} rg\n{ops}", rgb(color));
            (bbox, content)
        }
        NewAnnot::Note { at, text: body, color } => {
            let r = geom.display_rect([at.0, at.1, at.0 + 20.0, at.1 + 20.0]);
            d.set("Subtype", Object::Name("Text".into()));
            d.set("Name", Object::Name("Comment".into()));
            d.set("Contents", text(&body));
            d.set("C", reals(&color.map(|c| c as f32 / 255.0)));
            let [l, b, ..] = r;
            // A small note icon: filled card with an outline and three text lines.
            let mut c = format!(
                "{} rg 0.25 0.25 0.25 RG 0.8 w\n{} {} 18 18 re B\n0.8 w\n",
                rgb(color),
                num(l + 1.0),
                num(b + 1.0)
            );
            for (i, len) in [12.0, 12.0, 8.0].iter().enumerate() {
                let y = b + 14.5 - i as f32 * 4.0;
                c += &format!("{} {} m {} {} l S\n", num(l + 4.0), num(y), num(l + 4.0 + len), num(y));
            }
            (r, c)
        }
        NewAnnot::Ink { strokes, color, width } => {
            let strokes: Vec<Vec<(f32, f32)>> = strokes
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.into_iter().map(|(x, y)| geom.unpoint(x, y)).collect())
                .collect();
            if strokes.is_empty() {
                return Err("Nothing drawn".into());
            }
            let pad = width / 2.0 + 1.0;
            let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            let mut c = format!("{} RG {} w 1 J 1 j\n", rgb(color), num(width));
            let mut ink_list = Vec::new();
            for s in &strokes {
                let mut flat = Vec::new();
                for (i, &(x, y)) in s.iter().enumerate() {
                    b = [b[0].min(x - pad), b[1].min(y - pad), b[2].max(x + pad), b[3].max(y + pad)];
                    c += &format!("{} {} {}\n", num(x), num(y), if i == 0 { "m" } else { "l" });
                    flat.extend_from_slice(&[x, y]);
                }
                if s.len() == 1 {
                    // A dot: zero-length segment drawn with round caps.
                    c += &format!("{} {} l\n", num(s[0].0 + 0.01), num(s[0].1));
                }
                c += "S\n";
                ink_list.push(reals(&flat));
            }
            d.set("Subtype", Object::Name("Ink".into()));
            d.set("InkList", Object::Array(ink_list));
            d.set("C", reals(&color.map(|c| c as f32 / 255.0)));
            d.set("BS", dictionary! { "W" => Object::Real(width) });
            (b, c)
        }
        NewAnnot::Shape { kind, rect, color, width } => {
            let r = geom.display_rect(rect);
            let pad = width / 2.0 + 1.0;
            let bbox = [r[0] - pad, r[1] - pad, r[2] + pad, r[3] + pad];
            let mut c = format!("{} RG {} w 1 j\n", rgb(color), num(width));
            match kind {
                ShapeKind::Rectangle => {
                    d.set("Subtype", Object::Name("Square".into()));
                    c += &format!("{} {} {} {} re S\n", num(r[0]), num(r[1]), num(r[2] - r[0]), num(r[3] - r[1]));
                }
                ShapeKind::Ellipse => {
                    d.set("Subtype", Object::Name("Circle".into()));
                    // Four cubic Béziers approximating the ellipse.
                    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
                    let (rx, ry) = ((r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
                    let k = 0.552_284_8;
                    let p = |x: f32, y: f32| format!("{} {}", num(x), num(y));
                    c += &format!("{} m\n", p(cx + rx, cy));
                    c += &format!("{} {} {} c\n", p(cx + rx, cy + k * ry), p(cx + k * rx, cy + ry), p(cx, cy + ry));
                    c += &format!("{} {} {} c\n", p(cx - k * rx, cy + ry), p(cx - rx, cy + k * ry), p(cx - rx, cy));
                    c += &format!("{} {} {} c\n", p(cx - rx, cy - k * ry), p(cx - k * rx, cy - ry), p(cx, cy - ry));
                    c += &format!("{} {} {} c\n", p(cx + k * rx, cy - ry), p(cx + rx, cy - k * ry), p(cx + rx, cy));
                    c += "S\n";
                }
            }
            d.set("C", reals(&color.map(|c| c as f32 / 255.0)));
            d.set("BS", dictionary! { "W" => Object::Real(width) });
            (bbox, c)
        }
    };
    d.set("Rect", reals(&bbox));
    let ap = lopdf::Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => reals(&bbox),
            "Resources" => resources,
        },
        content.into_bytes(),
    );
    Ok((d, ap))
}

/// Moves/resizes an annotation to a displayed rect. Its appearance is mapped
/// into the new rect by the viewer, so ink and shapes scale with it.
pub fn set_rect(doc: &PdfDocument, page_index: u16, geom: &Geom, id: usize, rect: [f32; 4]) -> Result<(), String> {
    let page = doc.pages().get(page_index).map_err(err)?;
    let mut a = page.annotations().get(id as PdfPageAnnotationIndex).map_err(err)?;
    a.set_bounds(pdf_rect(geom.display_rect(rect))).map_err(err)?;
    a.set_modification_date(chrono::Utc::now()).map_err(err)
}

pub fn set_contents(doc: &PdfDocument, page_index: u16, id: usize, text: &str) -> Result<(), String> {
    let page = doc.pages().get(page_index).map_err(err)?;
    let mut a = page.annotations().get(id as PdfPageAnnotationIndex).map_err(err)?;
    a.set_contents(text).map_err(err)?;
    a.set_modification_date(chrono::Utc::now()).map_err(err)
}

pub fn delete(doc: &PdfDocument, page_index: u16, id: usize) -> Result<(), String> {
    let mut page = doc.pages().get(page_index).map_err(err)?;
    let a = page.annotations().get(id as PdfPageAnnotationIndex).map_err(err)?;
    page.annotations_mut().delete_annotation(a).map_err(err)
}
