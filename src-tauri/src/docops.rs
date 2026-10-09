//! Whole-document operations: export pages as images, properties,
//! password protection and compression.
//!
//! Password-protected PDFs are decrypted in memory when opened (`decrypt`),
//! so every editing feature works on them; `Protection` is applied again when
//! the document is saved (`encrypt`).

use crate::engine::err;
use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{Dictionary, Object};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

fn lo(e: lopdf::Error) -> String {
    format!("PDF structure error: {e}")
}

// ---- Export pages as images ----

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
}

/// Renders pages to `dir` as `<stem> - page N.png|jpg`; returns the files written.
pub fn export_images(
    doc: &PdfDocument,
    pages: &[u16],
    dpi: f32,
    format: ImageFormat,
    dir: &Path,
    stem: &str,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for &i in pages {
        let page = doc.pages().get(i).map_err(err)?;
        let width = (page.width().value / 72.0 * dpi).round().clamp(16.0, 20000.0) as i32;
        let config = PdfRenderConfig::new().set_target_width(width).render_form_data(true).render_annotations(true);
        let bitmap = page.render_with_config(&config).map_err(err)?;
        let img = bitmap.as_image();
        let (ext, fmt) = match format {
            ImageFormat::Png => ("png", image::ImageFormat::Png),
            ImageFormat::Jpeg => ("jpg", image::ImageFormat::Jpeg),
        };
        let path = dir.join(format!("{stem} - page {}.{ext}", i + 1));
        let img = match format {
            ImageFormat::Jpeg => image::DynamicImage::ImageRgb8(img.to_rgb8()),
            ImageFormat::Png => img,
        };
        img.save_with_format(&path, fmt).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        out.push(path.to_string_lossy().into_owned());
    }
    Ok(out)
}

// ---- Properties ----

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    // Read-only details
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub producer: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub modified: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub page_count: u16,
    #[serde(default)]
    pub page_size: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub protected: bool,
}

/// Formats a PDF date (D:YYYYMMDDHHmmSS...) for display.
fn pdf_date(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    if d.len() >= 12 && d[..12].chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{} {}:{}", &d[0..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12])
    } else {
        s.to_string()
    }
}

pub fn properties(doc: &PdfDocument, path: &str, protected: bool) -> Properties {
    let meta = doc.metadata();
    let get = |t: PdfDocumentMetadataTagType| meta.get(t).map(|v| v.value().to_string()).unwrap_or_default();
    let size = doc.pages().get(0).ok().map(|p| {
        let (w, h) = (p.width().value, p.height().value);
        let name = match ((w.min(h)).round() as i32, (w.max(h)).round() as i32) {
            (595, 842) => " (A4)",
            (612, 792) => " (Letter)",
            (612, 1008) => " (Legal)",
            (842, 1191) => " (A3)",
            (420, 595) => " (A5)",
            _ => "",
        };
        format!("{:.0} × {:.0} mm{name}", w / 72.0 * 25.4, h / 72.0 * 25.4)
    });
    Properties {
        title: get(PdfDocumentMetadataTagType::Title),
        author: get(PdfDocumentMetadataTagType::Author),
        subject: get(PdfDocumentMetadataTagType::Subject),
        keywords: get(PdfDocumentMetadataTagType::Keywords),
        creator: get(PdfDocumentMetadataTagType::Creator),
        producer: get(PdfDocumentMetadataTagType::Producer),
        created: pdf_date(&get(PdfDocumentMetadataTagType::CreationDate)),
        modified: pdf_date(&get(PdfDocumentMetadataTagType::ModificationDate)),
        version: match doc.version() {
            PdfDocumentVersion::Pdf1_0 => "1.0",
            PdfDocumentVersion::Pdf1_1 => "1.1",
            PdfDocumentVersion::Pdf1_2 => "1.2",
            PdfDocumentVersion::Pdf1_3 => "1.3",
            PdfDocumentVersion::Pdf1_4 => "1.4",
            PdfDocumentVersion::Pdf1_5 => "1.5",
            PdfDocumentVersion::Pdf1_6 => "1.6",
            PdfDocumentVersion::Pdf1_7 => "1.7",
            PdfDocumentVersion::Pdf2_0 => "2.0",
            _ => "",
        }
        .to_string(),
        page_count: doc.pages().len(),
        page_size: size.unwrap_or_default(),
        file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        protected,
    }
}

/// A PDF text string: ASCII as-is, otherwise UTF-16BE with BOM.
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

/// Writes title/author/subject/keywords to the Info dictionary (incremental update).
pub fn set_properties(bytes: &[u8], p: &Properties) -> Result<Vec<u8>, String> {
    let prev = lopdf::Document::load_mem(bytes).map_err(lo)?;
    let info_id = prev.trailer.get(b"Info").and_then(Object::as_reference).ok();
    let mut inc = lopdf::IncrementalDocument::create_from(bytes.to_vec(), prev);
    let info_id = match info_id {
        Some(id) => {
            inc.opt_clone_object_to_new_document(id).map_err(lo)?;
            id
        }
        None => {
            let id = inc.new_document.add_object(Dictionary::new());
            inc.new_document.trailer.set("Info", Object::Reference(id));
            id
        }
    };
    let info = inc.new_document.get_object_mut(info_id).and_then(Object::as_dict_mut).map_err(lo)?;
    for (key, value) in [("Title", &p.title), ("Author", &p.author), ("Subject", &p.subject), ("Keywords", &p.keywords)] {
        if value.trim().is_empty() {
            info.remove(key.as_bytes());
        } else {
            info.set(key, text(value.trim()));
        }
    }
    info.set("ModDate", text(&chrono::Utc::now().format("D:%Y%m%d%H%M%SZ").to_string()));
    let mut out = Vec::with_capacity(bytes.len() + 2048);
    inc.save_to(&mut out).map_err(|e| format!("Could not write the properties: {e}"))?;
    Ok(out)
}

// ---- Password protection ----

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Protection {
    /// Password needed to open the document ("" = opens freely, restrictions only).
    pub user_password: String,
    /// Password needed to change restrictions; random when not given.
    #[serde(default)]
    pub owner_password: String,
    pub allow_print: bool,
    pub allow_copy: bool,
    pub allow_edit: bool,
}

/// Decrypts a password-protected PDF in memory. Returns the plain bytes and
/// the protection to re-apply on save.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<(Vec<u8>, Protection), String> {
    // load_mem() does not load the objects of an encrypted file; the
    // password-aware loader decrypts them while parsing.
    let doc = lopdf::Document::load_mem_with_password(bytes, password).map_err(|e| format!("Could not decrypt: {e}"))?;
    let perms = doc
        .encryption_state
        .as_ref()
        .map(|s| s.permissions())
        .unwrap_or(lopdf::Permissions::all());
    let mut doc = doc;
    doc.encryption_state = None;
    doc.trailer.remove(b"Encrypt");
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out).map_err(|e| format!("Could not write the PDF: {e}"))?;
    let protection = Protection {
        user_password: password.to_string(),
        owner_password: String::new(),
        allow_print: perms.contains(lopdf::Permissions::PRINTABLE),
        allow_copy: perms.contains(lopdf::Permissions::COPYABLE),
        allow_edit: perms.contains(lopdf::Permissions::MODIFIABLE),
    };
    Ok((out, protection))
}

fn random_password() -> String {
    let mut b = [0u8; 18];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Encrypts plain PDF bytes with AES-256 (PDF 2.0, revision 6).
pub fn encrypt(bytes: &[u8], p: &Protection) -> Result<Vec<u8>, String> {
    let mut doc = lopdf::Document::load_mem(bytes).map_err(lo)?;
    let mut perms = lopdf::Permissions::COPYABLE_FOR_ACCESSIBILITY;
    if p.allow_print {
        perms |= lopdf::Permissions::PRINTABLE | lopdf::Permissions::PRINTABLE_IN_HIGH_QUALITY;
    }
    if p.allow_copy {
        perms |= lopdf::Permissions::COPYABLE;
    }
    if p.allow_edit {
        perms |= lopdf::Permissions::MODIFIABLE | lopdf::Permissions::ANNOTABLE | lopdf::Permissions::FILLABLE | lopdf::Permissions::ASSEMBLABLE;
    }
    let owner = if p.owner_password.is_empty() { random_password() } else { p.owner_password.clone() };
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| format!("No secure random source: {e}"))?;
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = lopdf::EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: &owner,
        user_password: &p.user_password,
        permissions: perms,
    };
    let state = lopdf::EncryptionState::try_from(version).map_err(lo)?;
    doc.encrypt(&state).map_err(lo)?;
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out).map_err(|e| format!("Could not write the PDF: {e}"))?;
    Ok(out)
}

// ---- Compression ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressReport {
    pub before: u64,
    pub after: u64,
    pub images_resampled: u32,
}

/// Downsamples images above `max_dpi` to JPEG (opaque images only), returning
/// how many were replaced. Works on the open PDFium document.
pub fn downsample_images(doc: &PdfDocument, max_dpi: f32, quality: u8) -> Result<u32, String> {
    let mut count = 0;
    for page_index in 0..doc.pages().len() {
        let mut page = doc.pages().get(page_index).map_err(err)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let mut jobs = Vec::new();
        for (i, obj) in page.objects().iter().enumerate() {
            let PdfPageObject::Image(im) = &obj else { continue };
            if obj.has_transparency() {
                continue;
            }
            let (Ok(pw), Ok(ph)) = (im.width(), im.height()) else { continue };
            let Ok(m) = obj.matrix() else { continue };
            let (wpt, hpt) = (m.a().hypot(m.b()), m.c().hypot(m.d()));
            if wpt < 1.0 || hpt < 1.0 {
                continue;
            }
            let dpi = (pw as f32 / (wpt / 72.0)).min(ph as f32 / (hpt / 72.0));
            if dpi <= max_dpi * 1.15 {
                continue;
            }
            let Ok(img) = im.get_raw_image() else { continue };
            let old_len = im.get_raw_image_data().map(|d| d.len()).unwrap_or(usize::MAX);
            let scale = max_dpi / dpi;
            let (nw, nh) = (((pw as f32) * scale).round().max(1.0) as u32, ((ph as f32) * scale).round().max(1.0) as u32);
            let small = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3).to_rgb8();
            let mut jpeg = Vec::new();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
            if image::DynamicImage::ImageRgb8(small).write_with_encoder(enc).is_err() {
                continue;
            }
            if jpeg.len() as f32 > old_len as f32 * 0.9 {
                continue; // not worth it
            }
            jobs.push((i, m, jpeg));
        }
        // Replace from the end so indices stay valid; keep z-order and placement.
        for (i, m, jpeg) in jobs.into_iter().rev() {
            let mut new_obj = PdfPageImageObject::new_from_jpeg_reader(doc, Cursor::new(jpeg)).map_err(err)?;
            new_obj.apply_matrix(m).map_err(err)?;
            page.objects_mut()
                .insert_object_at_index(i as PdfPageObjectIndex, PdfPageObject::Image(new_obj))
                .map_err(err)?;
            // See edit.rs: removed objects are leaked to avoid a double destroy.
            std::mem::forget(page.objects_mut().remove_object_at_index((i + 1) as PdfPageObjectIndex).map_err(err)?);
            count += 1;
        }
        if count > 0 {
            page.regenerate_content().map_err(err)?;
        }
    }
    Ok(count)
}

/// Removes unused objects and repacks the file with compressed object streams.
pub fn repack(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut doc = lopdf::Document::load_mem(bytes).map_err(lo)?;
    doc.prune_objects();
    doc.delete_zero_length_streams();
    doc.compress();
    let options = lopdf::SaveOptions::builder().use_object_streams(true).use_xref_streams(true).compression_level(9).build();
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_with_options(&mut out, options).map_err(|e| format!("Could not write the PDF: {e}"))?;
    // Never make a file bigger.
    Ok(if out.len() < bytes.len() { out } else { bytes.to_vec() })
}

