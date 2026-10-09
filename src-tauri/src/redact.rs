//! True redaction: content under the marked areas is removed from the page,
//! not just covered.
//!
//! - Text: characters touching an area are deleted. A text object that is
//!   only partly inside is rebuilt from its remaining characters, each placed
//!   at its original position with the original font, size and colour.
//! - Images: fully covered images are removed; partly covered ones have the
//!   covered pixels replaced with black in the image data itself.
//! - Vector shapes, shadings and form XObjects entirely inside an area are
//!   removed. (Partly covered ones are kept; they carry no text.)
//! - Annotations and form widgets touching an area are deleted.
//! - Finally a black box is drawn over each area.

use crate::engine::{err, Geom};
use pdfium_render::prelude::*;

/// [l, b, r, t] user-space rects intersect (with a tiny tolerance).
fn hits(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] - 0.01 && b[0] < a[2] - 0.01 && a[1] < b[3] - 0.01 && b[1] < a[3] - 0.01
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5 && inner[1] >= outer[1] - 0.5 && inner[2] <= outer[2] + 0.5 && inner[3] <= outer[3] + 0.5
}

fn rect4(r: &PdfRect) -> [f32; 4] {
    [r.left().value, r.bottom().value, r.right().value, r.top().value]
}

enum Action {
    Remove(usize),
    /// Re-create the text object's kept characters: (char, origin x, origin y).
    Rebuild { index: usize, keep: Vec<(String, f32, f32)>, font: PdfFontToken, size: f32, m: [f32; 4], color: PdfColor },
    /// Replace an image with a copy whose covered pixels are black.
    Mask { index: usize, image: image::DynamicImage, m: PdfMatrix },
}

/// Redacts displayed-page-space rects on one page. Returns how many page
/// objects and annotations were removed or altered.
pub fn redact(doc: &PdfDocument, page_index: u16, geom: &Geom, rects: &[[f32; 4]]) -> Result<usize, String> {
    let areas: Vec<[f32; 4]> = rects.iter().map(|&r| geom.display_rect(r)).filter(|r| r[2] > r[0] && r[3] > r[1]).collect();
    if areas.is_empty() {
        return Ok(0);
    }
    let mut page = doc.pages().get(page_index).map_err(err)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

    // 1. Plan, while page objects and the text page are borrowed.
    let mut actions = Vec::new();
    {
        let text_page = page.text().map_err(err)?;
        for (index, obj) in page.objects().iter().enumerate() {
            let Ok(b) = obj.bounds().map(|q| rect4(&q.to_rect())) else { continue };
            if !areas.iter().any(|&a| hits(b, a)) {
                continue;
            }
            match &obj {
                PdfPageObject::Text(t) => {
                    let chars = text_page.chars_for_object(t).map_err(err)?;
                    let mut keep = Vec::new();
                    let mut removed = false;
                    for ch in chars.iter() {
                        let Some(s) = ch.unicode_string() else { continue };
                        let Ok((ox, oy)) = ch.origin() else { continue };
                        let hit = match ch.tight_bounds() {
                            Ok(cb) if cb.width().value > 0.0 => areas.iter().any(|&a| hits(rect4(&cb), a)),
                            // Spaces and other zero-size glyphs: judged by their origin.
                            _ => areas.iter().any(|a| ox.value >= a[0] && ox.value <= a[2] && oy.value >= a[1] && oy.value <= a[3]),
                        };
                        if hit {
                            removed = true;
                        } else {
                            keep.push((s, ox.value, oy.value));
                        }
                    }
                    if !removed {
                        continue;
                    }
                    if keep.iter().all(|(s, ..)| s.trim().is_empty()) {
                        actions.push(Action::Remove(index));
                    } else {
                        let m = t.matrix().map_err(err)?;
                        actions.push(Action::Rebuild {
                            index,
                            keep,
                            font: t.font().token(),
                            size: t.unscaled_font_size().value,
                            m: [m.a(), m.b(), m.c(), m.d()],
                            color: obj.fill_color().unwrap_or(PdfColor::new(0, 0, 0, 255)),
                        });
                    }
                }
                PdfPageObject::Image(im) => {
                    if areas.iter().any(|&a| inside(b, a)) {
                        actions.push(Action::Remove(index));
                        continue;
                    }
                    let m = obj.matrix().map_err(err)?;
                    let mut img = image::DynamicImage::ImageRgba8(im.get_raw_image().map_err(err)?.to_rgba8());
                    let (pw, ph) = (img.width() as f32, img.height() as f32);
                    let inv = m.invert();
                    for &a in &areas {
                        // Area corners -> unit square (y up) -> pixels (y down).
                        let corners = [(a[0], a[1]), (a[2], a[1]), (a[0], a[3]), (a[2], a[3])].map(|(x, y)| {
                            let (u, v) = inv.apply_to_points(PdfPoints::new(x), PdfPoints::new(y));
                            (u.value * pw, (1.0 - v.value) * ph)
                        });
                        let x0 = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
                        let x1 = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).ceil().min(pw) as u32;
                        let y0 = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
                        let y1 = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).ceil().min(ph) as u32;
                        if let image::DynamicImage::ImageRgba8(buf) = &mut img {
                            for y in y0..y1 {
                                for x in x0..x1 {
                                    buf.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                                }
                            }
                        }
                    }
                    actions.push(Action::Mask { index, image: img, m });
                }
                _ => {
                    if areas.iter().any(|&a| inside(b, a)) {
                        actions.push(Action::Remove(index));
                    }
                }
            }
        }
    }
    let changed = actions.len();

    // 2. Apply from the highest index down so earlier indices stay valid.
    actions.sort_by_key(|a| std::cmp::Reverse(match a {
        Action::Remove(i) | Action::Rebuild { index: i, .. } | Action::Mask { index: i, .. } => *i,
    }));
    for action in actions {
        match action {
            Action::Remove(i) => {
                // See edit.rs: removed objects are leaked to avoid a double destroy.
                std::mem::forget(page.objects_mut().remove_object_at_index(i as PdfPageObjectIndex).map_err(err)?);
            }
            Action::Rebuild { index, keep, font, size, m, color } => {
                let n = keep.len();
                for (k, (s, x, y)) in keep.into_iter().enumerate() {
                    let mut o = PdfPageTextObject::new(doc, &s, font, PdfPoints::new(size)).map_err(err)?;
                    o.set_fill_color(color).map_err(err)?;
                    o.apply_matrix(PdfMatrix::new(m[0], m[1], m[2], m[3], x, y)).map_err(err)?;
                    page.objects_mut()
                        .insert_object_at_index((index + k) as PdfPageObjectIndex, PdfPageObject::Text(o))
                        .map_err(err)?;
                }
                std::mem::forget(page.objects_mut().remove_object_at_index((index + n) as PdfPageObjectIndex).map_err(err)?);
            }
            Action::Mask { index, image, m } => {
                let mut o = PdfPageImageObject::new(doc, &image).map_err(err)?;
                o.apply_matrix(m).map_err(err)?;
                page.objects_mut()
                    .insert_object_at_index(index as PdfPageObjectIndex, PdfPageObject::Image(o))
                    .map_err(err)?;
                std::mem::forget(page.objects_mut().remove_object_at_index((index + 1) as PdfPageObjectIndex).map_err(err)?);
            }
        }
    }

    // 3. Annotations and form widgets touching an area.
    let doomed: Vec<usize> = page
        .annotations()
        .iter()
        .enumerate()
        .filter(|(_, a)| a.bounds().is_ok_and(|b| areas.iter().any(|&r| hits(rect4(&b), r))))
        .map(|(i, _)| i)
        .collect();
    for &i in doomed.iter().rev() {
        let a = page.annotations().get(i as PdfPageAnnotationIndex).map_err(err)?;
        page.annotations_mut().delete_annotation(a).map_err(err)?;
    }

    // 4. Black boxes.
    for a in &areas {
        let rect = PdfRect::new_from_values(a[1], a[0], a[3], a[2]);
        let path = PdfPagePathObject::new_rect(doc, rect, None, None, Some(PdfColor::new(0, 0, 0, 255))).map_err(err)?;
        page.objects_mut().add_path_object(path).map_err(err)?;
    }
    page.regenerate_content().map_err(err)?;
    Ok(changed + doomed.len())
}
