mod engine;

use engine::{DocId, DocInfo, Engine, Link, OutlineItem, PageText, SearchHit};
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
            take_startup_files
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
