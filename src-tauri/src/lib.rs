mod annots;
mod edit;
mod engine;
mod forms;

use engine::{DocId, DocInfo, DocState, Engine, Link, OutlineItem, PageText, SearchHit, Target};
use std::sync::{Arc, Mutex};
use tauri::{ipc::Response, AppHandle, Emitter, Manager, State};

/// PDF paths passed on the command line at startup (e.g. "Open with").
struct StartupFiles(Mutex<Vec<String>>);

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
async fn add_image(engine: State<'_, Engine>, id: DocId, page: u16, x: f32, y: f32, path: String) -> Result<DocState, String> {
    engine.add_image(id, page, (x, y), edit::ImageSource::Path(path), None).await
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
    let cwd = std::env::current_dir().unwrap_or_default();
    tauri::Builder::default()
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .manage(StartupFiles(Mutex::new(pdf_args(std::env::args(), &cwd))))
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
            fill_fields
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
