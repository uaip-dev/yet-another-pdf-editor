# Roadmap

Open source (MIT), funded by support and donations. Windows is the first platform;
macOS and Linux come from the same codebase later.

## Stack
- **Shell:** Tauri 2 (Rust + system webview). Installer target: < 20 MB.
- **UI:** Svelte 5 + TypeScript (SvelteKit in SPA mode).
- **Engine:** PDFium through `pdfium-render`, run on one dedicated worker thread
  (`src-tauri/src/engine.rs`). PDFium binaries come from bblanchon/pdfium-binaries,
  pinned in `scripts/fetch-pdfium.ps1`.
- **Low-level fallback:** `lopdf` for content-stream work that PDFium does not expose.

## Phases
Editing text and images is the selling point, so it comes right after the viewer.

| Phase | Scope | Status |
|---|---|---|
| 0 | Project setup, PDFium binding, render pages, open/drag-drop, password PDFs, zoom | **done** |
| 1 | Viewer: thumbnails, outline, text select/copy, search, links, print, recent files, file association, tabs | next |
| 2 | **Editing:** edit existing text, add text, replace/move/resize/delete/add images, undo/redo, save/save-as | |
| 3 | Annotations and forms: highlight, underline, strikeout, notes, ink, shapes, stamps, AcroForm fill, signatures | |
| 4 | Page management: insert, delete, reorder, rotate, extract, merge, split | |
| 5 | Polish: export images, compress, encrypt, properties, true redaction, macOS + Linux builds, auto-update | |
| Later | OCR (Tesseract), digital certificate signatures, compare | |

## How text editing works (Phase 2)
1. **Detect:** enumerate text page objects; group them into lines and paragraphs by baseline, font and size.
2. **Edit:** show a matching inline editor overlay (same font, size, colour, position).
3. **Write back:**
   - The embedded font has every glyph needed → modify the text object in place.
   - The font is a subset or glyphs are missing → remove the original objects and redraw with a
     matching system font, falling back to a bundled Noto font. Tell the user when a font is substituted.
   - Reflow within the paragraph box only, never across the whole page.
4. **Images:** swap the image object's bitmap and keep its transform matrix.

## Performance targets
- Cold start < 1 s; first page < 300 ms; < 150 MB RAM with a 100-page document open.
- Only pages near the viewport are rendered; canvases far from view are released.

## Known risks
- Subset fonts → handled by font substitution (see above).
- XFA forms → PDFium has partial support; show them read-only and say so.
- Saving → write to a temp file and swap atomically; keep a backup of the original.
