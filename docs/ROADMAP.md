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
| 1 | Viewer: thumbnails, outline, text select/copy, search, links, print, recent files, file association, tabs | **done** |
| 2 | **Editing:** edit existing text, add text, replace/move/resize/delete/add images, undo/redo, save/save-as | **done** |
| 3 | Annotations and forms: highlight, underline, strikeout, notes, ink, shapes, stamps, AcroForm fill, signatures | next |
| 4 | Page management: insert, delete, reorder, rotate, extract, merge, split | |
| 5 | Polish: export images, compress, encrypt, properties, true redaction, macOS + Linux builds, auto-update | |
| Later | OCR (Tesseract), digital certificate signatures, compare | |

## How text editing works (Phase 2)
Code: `src-tauri/src/edit.rs`.

1. **Blocks.** Many PDFs (Chromium/Skia output especially) store each glyph as its own text
   object. Glyphs are grouped into lines (shared baseline, small gaps) and lines into paragraphs
   (same left edge, size and line spacing). A block is the editing unit.
2. **Styles survive edits.** The new text is diffed against the old (common prefix/suffix), so
   unchanged characters keep their font, size and colour; inserted text takes its neighbour's.
3. **Fonts.** Embedded subset fonts only contain glyphs the document already uses. Each character
   keeps the original font when that font has the glyph, and otherwise falls back to a matching
   standard font (Helvetica/Times/Courier, bold/italic as needed), or a system TrueType font for
   characters outside WinAnsi. The UI says which characters were substituted.
4. **Layout.** Paragraphs re-wrap at their original width, measured with our own glyph advances
   against the original lines, so unchanged text wraps exactly where it did. When the original
   was set tighter than the font's advances (Chromium does this), glyphs are positioned
   individually so spacing matches the original. Single-line blocks grow sideways.
5. **Images.** Replace keeps the frame, rotation and z-order and fits the new picture inside it
   (aspect ratio preserved). JPEGs are embedded as-is. Move/resize/delete work for images and blocks.
6. **Undo/redo** keep whole-document snapshots (50 steps / 256 MB). **Save** writes a temp file
   and renames it over the target; the previous file is copied to the app's `backups` folder.

### Known limits
- Rotated/skewed text and text inside form XObjects is not editable yet.
- Edited paragraphs become left-aligned (justified/centred alignment is not reproduced).
- The inline editor previews with a generic font of the same family, not the embedded font.
- pdfium-render 0.8.37 double-destroys removed page objects, so they are leaked deliberately
  (`mem::forget`), a few hundred bytes per removed glyph per session.

## Performance targets
- Cold start < 1 s; first page < 300 ms; < 150 MB RAM with a 100-page document open.
- Only pages near the viewport are rendered; canvases far from view are released.

## Known risks
- Subset fonts → handled by font substitution (see above).
- XFA forms → PDFium has partial support; show them read-only and say so.
- Saving → write to a temp file and swap atomically; keep a backup of the original.
