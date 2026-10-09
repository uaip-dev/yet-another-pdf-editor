# Extensions

The app can be extended by a separate build without changing this repository.
The Community edition uses all of these extension points with nothing plugged in.

## Backend

The app is a library (`yape_lib`). A different binary can start it with extra
[Tauri plugins](https://v2.tauri.app/develop/plugins/):

```rust
fn main() {
    yape_lib::run_with(tauri::generate_context!(), |builder| {
        builder.plugin(my_extension::init())
    });
}
```

Inside a plugin command, work on an open document through the engine. Changes
made this way get undo, the unsaved-changes marker and a UI refresh like any
built-in edit:

```rust
use yape_lib::engine::{DocAccess, DocId, DocState, Engine};

#[tauri::command]
async fn my_command(engine: tauri::State<'_, Engine>, id: DocId) -> Result<DocState, String> {
    engine
        .with_document(id, |doc: &mut DocAccess| {
            // Either edit doc.doc_mut() between begin_change() and finish_change() ...
            doc.begin_change()?;
            /* ... */
            doc.finish_change();
            // ... or build new PDF bytes and call doc.replace(bytes).
            Ok(doc.state())
        })
        .await
}
```

Register the feature names your plugin provides so the UI can show them:

```rust
app.state::<yape_lib::Capabilities>().add("my-feature");
```

## Frontend

The UI imports three slot components from the `$pro` alias:

| Component | Mounted | Use it for |
|---|---|---|
| `MoreMenu.svelte` | inside the toolbar's More menu | extra menu items (`<button role="menuitem">`) |
| `Start.svelte` | on the start screen (no document open) | actions that don't need a document |
| `Host.svelte` | always | dialogs, panels and other UI |

Both receive `ctx: ExtensionContext` (see `src/lib/extension.ts`): the active document,
the registered capabilities, and callbacks to apply a returned `DocState`, show a notice
or report an error. `MoreMenu` also receives `close()`.

Community builds use the empty components in `src/lib/pro/`. To use your own, copy them
into this project's `.pro-ui/` folder (git-ignored) and point the alias at it:

```bash
YAPE_PRO_UI=.pro-ui pnpm tauri build
```

Keeping the components inside the project lets them import anything from `$lib` (API calls,
icons, styles) and use the same `svelte` dependency.
