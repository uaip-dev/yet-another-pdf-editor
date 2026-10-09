//! Yet Another PDF Editor.
//!
//! The app can be extended without changing this crate: `run_with` lets a
//! separate build add Tauri plugins, `engine::Engine::with_document` gives
//! them access to open documents (with undo), and `Capabilities` tells the UI
//! which extra features are present. See docs/EXTENSIONS.md.

mod annots;
mod docops;
mod edit;
pub mod engine;
mod forms;
mod pages;
mod redact;

pub use docops::{CompressReport, ImageFormat, Protection};
pub use pdfium_render;

use engine::{DocId, DocInfo, DocState, Engine, Link, OutlineItem, PageText, SearchHit, Target};
use std::sync::{Arc, Mutex};
use tauri::{ipc::Response, AppHandle, Emitter, Manager, State};

/// PDF paths passed on the command line at startup (e.g. "Open with").
struct StartupFiles(Mutex<Vec<String>>);

/// Names of optional features provided by extensions (e.g. "ocr"). The UI
/// asks for this list to decide which extra tools to show.
#[derive(Default)]
pub struct Capabilities(Mutex<Vec<String>>);

impl Capabilities {
    pub fn add(&self, name: &str) {
        let mut list = self.0.lock().unwrap();
        if !list.iter().any(|n| n == name) {
            list.push(name.to_string());
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

#[tauri::command]
fn capabilities(caps: State<'_, Capabilities>) -> Vec<String> {
    caps.list()
}

#[tauri::command]
async fn open_document(
    engine: State<'_, Engine>,
    path: String,
    password: Option<String>,
) -> Result<DocInfo, String> {
    engine.open(path, password).await
}

#[tauri::command]
async fn close_document(engine: State<'_, Engine>, id: DocId) -> Result<(), String> {
    engine.close(id).await;
    Ok(())
}

/// Returns raw RGBA bytes (see `engine::RenderedPage`) as a binary IPC response.
#[tauri::command]
async fn render_page(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    width: u32,
) -> Result<Response, String> {
    engine.render(id, page, width).await.map(Response::new)
}

#[tauri::command]
async fn page_text(engine: State<'_, Engine>, id: DocId, page: u16) -> Result<Arc<PageText>, String> {
    engine.page_text(id, page).await
}

#[tauri::command]
async fn page_links(engine: State<'_, Engine>, id: DocId, page: u16) -> Result<Vec<Link>, String> {
    engine.links(id, page).await
}

#[tauri::command]
async fn outline(engine: State<'_, Engine>, id: DocId) -> Result<Vec<OutlineItem>, String> {
    engine.outline(id).await
}

#[tauri::command]
async fn search(
    engine: State<'_, Engine>,
    id: DocId,
    query: String,
    match_case: bool,
) -> Result<Vec<SearchHit>, String> {
    engine.search(id, query, match_case).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LayoutResponse {
    revision: u32,
    #[serde(flatten)]
    layout: edit::PageLayout,
}

#[tauri::command]
async fn page_layout(engine: State<'_, Engine>, id: DocId, page: u16) -> Result<LayoutResponse, String> {
    let (revision, layout) = engine.page_layout(id, page).await?;
    Ok(LayoutResponse { revision, layout })
}

#[tauri::command]
async fn doc_state(engine: State<'_, Engine>, id: DocId) -> Result<DocState, String> {
    engine.state(id).await
}

#[tauri::command]
async fn edit_text(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    revision: u32,
    block: usize,
    text: String,
) -> Result<DocState, String> {
    engine.edit_text(id, page, revision, block, text).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn add_text(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    x: f32,
    y: f32,
    text: String,
    size: f32,
    family: String,
    bold: bool,
    italic: bool,
    color: [u8; 3],
) -> Result<DocState, String> {
    let color = (color[0], color[1], color[2]);
    engine.add_text(id, page, (x, y), text, size, family, bold, italic, color).await
}

#[tauri::command]
async fn transform_object(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    revision: u32,
    target: Target,
    index: usize,
    rect: [f32; 4],
) -> Result<DocState, String> {
    engine.transform(id, page, revision, target, index, rect).await
}

#[tauri::command]
async fn delete_object(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    revision: u32,
    target: Target,
    index: usize,
) -> Result<DocState, String> {
    engine.delete(id, page, revision, target, index).await
}

#[tauri::command]
async fn replace_image(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    revision: u32,
    index: usize,
    path: String,
) -> Result<DocState, String> {
    engine.replace_image(id, page, revision, index, path).await
}

#[tauri::command]
async fn add_image(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    x: f32,
    y: f32,
    path: String,
    width: Option<f32>,
) -> Result<DocState, String> {
    engine.add_image(id, page, (x, y), edit::ImageSource::Path(path), width).await
}

/// Places a signature (PNG bytes drawn or typed in the UI) as page content.
#[tauri::command]
async fn add_signature(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    x: f32,
    y: f32,
    png: Vec<u8>,
    width: f32,
) -> Result<DocState, String> {
    engine.add_image(id, page, (x, y), edit::ImageSource::Bytes(png), Some(width)).await
}

#[tauri::command]
async fn page_extras(engine: State<'_, Engine>, id: DocId, page: u16) -> Result<engine::PageExtras, String> {
    engine.page_extras(id, page).await
}

#[tauri::command]
async fn add_annotation(engine: State<'_, Engine>, id: DocId, page: u16, annot: annots::NewAnnot) -> Result<DocState, String> {
    engine.add_annotation(id, page, annot).await
}

#[tauri::command]
async fn change_annotation(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    revision: u32,
    annot: usize,
    change: engine::AnnotChange,
) -> Result<DocState, String> {
    engine.change_annotation(id, page, revision, annot, change).await
}

#[tauri::command]
async fn fill_fields(
    engine: State<'_, Engine>,
    id: DocId,
    page: u16,
    revision: u32,
    changes: Vec<(usize, forms::FieldValue)>,
) -> Result<DocState, String> {
    engine.fill_fields(id, page, revision, changes).await
}

#[tauri::command]
async fn undo(engine: State<'_, Engine>, id: DocId) -> Result<DocState, String> {
    engine.undo(id, false).await
}

#[tauri::command]
async fn redo(engine: State<'_, Engine>, id: DocId) -> Result<DocState, String> {
    engine.undo(id, true).await
}

/// Saves in place, or to `path` for "Save as". The previous file is backed up
/// to the app's local data folder first.
#[tauri::command]
async fn save_document(app: AppHandle, engine: State<'_, Engine>, id: DocId, path: Option<String>) -> Result<DocState, String> {
    let backups = app.path().app_local_data_dir().ok().map(|d| d.join("backups"));
    engine.save(id, path, backups).await
}

/// Reads an image file the user picked (e.g. a scanned signature) so the UI
/// can preview and prepare it. Only image types are allowed.
#[tauri::command]
async fn read_image_file(path: String) -> Result<Response, String> {
    let lower = path.to_lowercase();
    let ok = [".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"].iter().any(|e| lower.ends_with(e));
    if !ok {
        return Err("Not an image file".into());
    }
    std::fs::read(&path).map(Response::new).map_err(|e| e.to_string())
}

#[tauri::command]
async fn rotate_pages(engine: State<'_, Engine>, id: DocId, pages: Vec<u16>, delta: i32) -> Result<DocState, String> {
    engine.rotate_pages(id, pages, delta).await
}

#[tauri::command]
async fn delete_pages(engine: State<'_, Engine>, id: DocId, pages: Vec<u16>) -> Result<DocState, String> {
    engine.delete_pages(id, pages).await
}

#[tauri::command]
async fn move_pages(engine: State<'_, Engine>, id: DocId, pages: Vec<u16>, before: u16) -> Result<DocState, String> {
    engine.move_pages(id, pages, before).await
}

#[tauri::command]
async fn insert_blank_page(engine: State<'_, Engine>, id: DocId, at: u16, width: f32, height: f32) -> Result<DocState, String> {
    engine.insert_blank_page(id, at, width, height).await
}

#[tauri::command]
async fn insert_pages_from_file(
    engine: State<'_, Engine>,
    id: DocId,
    at: u16,
    path: String,
    password: Option<String>,
) -> Result<DocState, String> {
    engine.insert_pages_from_file(id, at, path, password).await
}

#[tauri::command]
async fn insert_image_page(engine: State<'_, Engine>, id: DocId, at: u16, path: String) -> Result<DocState, String> {
    engine.insert_image_page(id, at, path).await
}

#[tauri::command]
async fn extract_pages(engine: State<'_, Engine>, id: DocId, pages: Vec<u16>, out: String) -> Result<(), String> {
    engine.extract_pages(id, pages, out).await
}

#[tauri::command]
async fn split_document(engine: State<'_, Engine>, id: DocId, every: u16, dir: String) -> Result<Vec<String>, String> {
    engine.split(id, every, dir).await
}

#[tauri::command]
async fn document_properties(engine: State<'_, Engine>, id: DocId) -> Result<docops::Properties, String> {
    engine.properties(id).await
}

#[tauri::command]
async fn set_document_properties(engine: State<'_, Engine>, id: DocId, props: docops::Properties) -> Result<DocState, String> {
    engine.set_properties(id, props).await
}

#[tauri::command]
async fn document_protection(engine: State<'_, Engine>, id: DocId) -> Result<Option<docops::Protection>, String> {
    engine.protection(id).await
}

#[tauri::command]
async fn set_document_protection(
    engine: State<'_, Engine>,
    id: DocId,
    protection: Option<docops::Protection>,
) -> Result<DocState, String> {
    engine.set_protection(id, protection).await
}

#[tauri::command]
async fn export_images(
    engine: State<'_, Engine>,
    id: DocId,
    pages: Vec<u16>,
    dpi: f32,
    format: docops::ImageFormat,
    dir: String,
) -> Result<Vec<String>, String> {
    engine.export_images(id, pages, dpi, format, dir).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CompressResponse {
    state: DocState,
    report: docops::CompressReport,
}

#[tauri::command]
async fn compress_document(engine: State<'_, Engine>, id: DocId, max_dpi: f32, quality: u8) -> Result<CompressResponse, String> {
    let (state, report) = engine.compress(id, max_dpi, quality).await?;
    Ok(CompressResponse { state, report })
}

#[tauri::command]
async fn redact_areas(engine: State<'_, Engine>, id: DocId, page: u16, rects: Vec<[f32; 4]>) -> Result<DocState, String> {
    engine.redact(id, page, rects).await
}

/// Returns (once) the PDFs the app was launched with.
#[tauri::command]
fn take_startup_files(files: State<'_, StartupFiles>) -> Vec<String> {
    std::mem::take(&mut *files.0.lock().unwrap())
}

fn pdf_args(args: impl IntoIterator<Item = String>, cwd: &std::path::Path) -> Vec<String> {
    args.into_iter()
        .skip(1)
        .filter(|a| a.to_lowercase().ends_with(".pdf"))
        .map(|a| cwd.join(&a).to_string_lossy().into_owned())
        .collect()
}

/// Directories searched for the PDFium shared library, in priority order.
fn pdfium_dirs(app: &AppHandle) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        dirs.push(res);
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Into::into)) {
        dirs.push(exe_dir);
    }
    if cfg!(debug_assertions) {
        dirs.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("pdfium"));
    }
    dirs
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with(tauri::generate_context!(), |builder| builder);
}

/// Runs the app with a given context (app config) and lets `extend` add
/// plugins and state to the builder before it starts.
pub fn run_with<F>(context: tauri::Context<tauri::Wry>, extend: F)
where
    F: FnOnce(tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry>,
{
    let cwd = std::env::current_dir().unwrap_or_default();
    let builder = tauri::Builder::default()
        // Must be first: a second launch forwards its files here and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let files = pdf_args(argv, std::path::Path::new(&cwd));
            if !files.is_empty() {
                let _ = app.emit("open-files", files);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .manage(StartupFiles(Mutex::new(pdf_args(std::env::args(), &cwd))))
        .manage(Capabilities::default());
    extend(builder)
        .setup(|app| {
            app.manage(Engine::start(pdfium_dirs(app.handle())));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_document,
            close_document,
            render_page,
            page_text,
            page_links,
            outline,
            search,
            take_startup_files,
            page_layout,
            doc_state,
            edit_text,
            add_text,
            transform_object,
            delete_object,
            replace_image,
            add_image,
            undo,
            redo,
            save_document,
            add_signature,
            page_extras,
            add_annotation,
            change_annotation,
            fill_fields,
            read_image_file,
            rotate_pages,
            delete_pages,
            move_pages,
            insert_blank_page,
            insert_pages_from_file,
            insert_image_page,
            extract_pages,
            split_document,
            document_properties,
            set_document_properties,
            document_protection,
            set_document_protection,
            export_images,
            compress_document,
            redact_areas,
            capabilities
        ])
        .run(context)
        .expect("error while running tauri application");
}
