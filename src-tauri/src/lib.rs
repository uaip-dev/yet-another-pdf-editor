mod engine;

use engine::{DocId, DocInfo, Engine};
use tauri::{ipc::Response, Manager, State};

#[tauri::command]
async fn open_document(
    engine: State<'_, Engine>,
    path: String,
    password: Option<String>,
) -> Result<DocInfo, String> {
    engine.open(path, password).await
}

#[tauri::command]
fn close_document(engine: State<'_, Engine>, id: DocId) {
    engine.close(id);
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

/// Directories searched for the PDFium shared library, in priority order.
fn pdfium_dirs(app: &tauri::App) -> Vec<std::path::PathBuf> {
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .setup(|app| {
            app.manage(Engine::start(pdfium_dirs(app)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![open_document, close_document, render_page])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
