//! Form fields (AcroForm): listing and filling.
//!
//! Listing uses pdfium-render. Filling goes through PDFium's form-fill engine
//! (`FORM_*`), the same path an interactive viewer uses: it updates parent /
//! kid fields correctly and regenerates each widget's appearance, so the
//! values show up in every viewer. pdfium-render keeps its raw handles private,
//! so filling runs on a raw copy of the document (bytes in, bytes out) that
//! then replaces the open one, like an undo step.

use crate::engine::Geom;
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::os::raw::{c_int, c_ulong, c_void};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldInfo {
    /// Index among the page's annotations (the widget).
    pub id: usize,
    pub kind: &'static str,
    pub name: String,
    pub value: String,
    pub checked: bool,
    pub options: Vec<String>,
    pub selected: Option<usize>,
    /// left, top, right, bottom in displayed page space
    pub rect: [f32; 4],
    pub read_only: bool,
    pub multiline: bool,
    pub password: bool,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum FieldValue {
    Text(String),
    Checked(bool),
    /// Option index for combo and list boxes.
    Select(usize),
}

pub fn list(page: &PdfPage, geom: &Geom) -> Vec<FieldInfo> {
    let mut out = Vec::new();
    for (id, a) in page.annotations().iter().enumerate() {
        let Some(field) = a.as_form_field() else { continue };
        let Ok(b) = a.bounds() else { continue };
        let rect = geom.user_rect([b.left().value, b.bottom().value, b.right().value, b.top().value]);
        let mut info = FieldInfo {
            id,
            kind: "unknown",
            name: field.name().unwrap_or_default(),
            value: String::new(),
            checked: false,
            options: Vec::new(),
            selected: None,
            rect,
            read_only: field.is_read_only(),
            multiline: false,
            password: false,
        };
        match field.field_type() {
            PdfFormFieldType::Text => {
                let f = field.as_text_field().unwrap();
                info.kind = "text";
                info.value = f.value().unwrap_or_default();
                info.multiline = f.is_multiline();
                info.password = f.is_password();
            }
            PdfFormFieldType::Checkbox => {
                info.kind = "checkbox";
                info.checked = field.as_checkbox_field().and_then(|f| f.is_checked().ok()).unwrap_or(false);
            }
            PdfFormFieldType::RadioButton => {
                info.kind = "radio";
                // pdfium-render's is_checked() reports unselected radios as checked
                // (it compares two missing values). A radio is on when its group
                // value names this widget's current appearance state.
                info.checked = field.as_radio_button_field().is_some_and(|f| {
                    matches!((f.group_value(), field.appearance_stream()), (Some(v), Some(state)) if v == state && v != "Off")
                });
            }
            PdfFormFieldType::ComboBox | PdfFormFieldType::ListBox => {
                let (kind, value, opts) = if let Some(f) = field.as_combo_box_field() {
                    ("combo", f.value(), f.options())
                } else {
                    let f = field.as_list_box_field().unwrap();
                    ("list", f.value(), f.options())
                };
                info.kind = kind;
                info.value = value.unwrap_or_default();
                for o in opts.iter() {
                    if o.is_set() && info.selected.is_none() {
                        info.selected = Some(info.options.len());
                    }
                    info.options.push(o.label().cloned().unwrap_or_default());
                }
            }
            PdfFormFieldType::PushButton => info.kind = "button",
            PdfFormFieldType::Signature => info.kind = "signature",
            PdfFormFieldType::Unknown => {}
        }
        out.push(info);
    }
    out
}

// ---- Raw form filling ----

#[repr(C)]
struct Writer {
    base: FPDF_FILEWRITE,
    data: Vec<u8>,
}

unsafe extern "C" fn write_block(this: *mut FPDF_FILEWRITE, data: *const c_void, size: c_ulong) -> c_int {
    // SAFETY: `this` is the `base` field of a live, #[repr(C)] `Writer`.
    let w = this as *mut Writer;
    (*w).data.extend_from_slice(std::slice::from_raw_parts(data as *const u8, size as usize));
    1
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Applies field values to a document given as bytes and returns the new bytes.
pub fn fill(
    b: &dyn PdfiumLibraryBindings,
    bytes: &[u8],
    password: Option<&str>,
    page_index: u16,
    changes: &[(usize, FieldValue)],
) -> Result<Vec<u8>, String> {
    let doc = b.FPDF_LoadMemDocument64(bytes, password);
    if doc.is_null() {
        return Err("Could not reopen the document to fill its form".into());
    }
    // The fill info must outlive the form environment.
    let mut info: Box<FPDF_FORMFILLINFO> = Box::new(unsafe { std::mem::zeroed() });
    info.version = 1;
    let form = b.FPDFDOC_InitFormFillEnvironment(doc, &mut *info);
    let result = (|| {
        if form.is_null() {
            return Err("This document has no fillable form".to_string());
        }
        let page = b.FPDF_LoadPage(doc, page_index as c_int);
        if page.is_null() {
            return Err("Could not load page".to_string());
        }
        b.FORM_OnAfterLoadPage(page, form);
        let mut failed = None;
        for (id, value) in changes {
            let annot = b.FPDFPage_GetAnnot(page, *id as c_int);
            if annot.is_null() {
                failed = Some(format!("Form field {id} not found"));
                continue;
            }
            let mut r = FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
            b.FPDFAnnot_GetRect(annot, &mut r);
            let (cx, cy) = (((r.left + r.right) / 2.0) as f64, ((r.top + r.bottom) / 2.0) as f64);
            match value {
                FieldValue::Text(text) => {
                    b.FORM_SetFocusedAnnot(form, annot);
                    b.FORM_SelectAllText(form, page);
                    let w = wide(text);
                    b.FORM_ReplaceSelection(form, page, w.as_ptr());
                    b.FORM_ForceToKillFocus(form);
                }
                FieldValue::Checked(want) => {
                    let is = b.FPDFAnnot_IsChecked(form, annot) != 0;
                    if is != *want {
                        // Click it, as a user would; radio groups update their siblings.
                        b.FORM_OnMouseMove(form, page, 0, cx, cy);
                        b.FORM_OnLButtonDown(form, page, 0, cx, cy);
                        b.FORM_OnLButtonUp(form, page, 0, cx, cy);
                        b.FORM_ForceToKillFocus(form);
                    }
                }
                FieldValue::Select(index) => {
                    b.FORM_SetFocusedAnnot(form, annot);
                    b.FORM_SetIndexSelected(form, page, *index as c_int, 1);
                    b.FORM_ForceToKillFocus(form);
                }
            }
            b.FPDFPage_CloseAnnot(annot);
        }
        b.FORM_OnBeforeClosePage(page, form);
        b.FPDF_ClosePage(page);
        if let Some(e) = failed {
            return Err(e);
        }
        let mut writer = Box::new(Writer { base: FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) }, data: Vec::new() });
        let ok = b.FPDF_SaveAsCopy(doc, &mut writer.base, 0);
        if ok == 0 {
            return Err("Could not save the filled form".to_string());
        }
        Ok(std::mem::take(&mut writer.data))
    })();
    if !form.is_null() {
        b.FPDFDOC_ExitFormFillEnvironment(form);
    }
    b.FPDF_CloseDocument(doc);
    drop(info);
    result
}
