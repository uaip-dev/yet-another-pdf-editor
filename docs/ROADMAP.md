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
| 3 | Annotations and forms: highlight, underline, strikeout, notes, ink, shapes, AcroForm fill, signatures | **done** |
| 4 | Page management: insert, delete, reorder, rotate, extract, merge, split | **done** |
| 5 | Polish: export images, compress, encrypt, properties, true redaction, macOS + Linux builds, auto-update | **done** |
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

## Annotations, forms and signatures (Phase 3)
Code: `src-tauri/src/annots.rs`, `src-tauri/src/forms.rs`.

- **New annotations** are written with lopdf as an *incremental update*: the existing bytes are
  kept and only the annotation, its appearance stream and the updated page/`Annots` objects are
  appended. Each annotation is an indirect object with an explicit appearance (multiply-blended
  highlight, underline/strikeout lines, note icon, ink, rectangle, ellipse), so every viewer draws
  it the same. PDFium is not used to create them: it writes annotations as direct dictionaries
  (the spec requires indirect references; MuPDF ignores direct ones) and generates no appearance
  for text markup or notes.
- Listing, moving, editing note text and deleting use PDFium.
- **Forms** are listed via pdfium-render and filled through PDFium's form-fill engine (`FORM_*`)
  on a raw copy of the document, like a real viewer: parent/kid fields and widget appearances are
  updated, so filled values show in every viewer.
- **Signatures** are drawn, typed (handwriting fonts) or loaded from an image (with optional
  white-background removal), trimmed, and placed as an image with transparency. Up to five are
  remembered on the computer.

### Known limits
- Annotations can't be added to password-protected PDFs yet (lopdf can't update them incrementally).
- Text markup can't be moved (it belongs to its text); delete and re-create instead.
- Form calculation/format scripts (JavaScript) are not run.
- Signatures are visual (an image), not cryptographic digital signatures.
- pdfium-render bugs worked around: annotation colour getters crash PDFium on annotations with
  appearance streams (not used); radio buttons report "checked" when unselected (own check).

## Page management (Phase 4)
Code: `src-tauri/src/pages.rs`. Select pages in the thumbnail panel (click, Ctrl+click, Shift+click)
and use the toolbar above it, drag thumbnails to reorder, or press Del to delete.

- Rotate, delete, insert (blank page, pages from another PDF, image as page), extract to a new PDF
  and split every N pages all use PDFium, with undo; a failed operation rolls back.
- **Reordering** uses lopdf: the page tree is flattened (inherited Resources/MediaBox/CropBox/Rotate
  pushed down to each page) and rewritten as an incremental update, so bookmarks, links, form
  fields and metadata keep pointing at the right pages. Rebuilding the document in a new order
  with PDFium would have dropped them.
- Thumbnail reordering uses pointer events: Tauri's file-drop handling on Windows disables HTML5
  drag and drop inside the page.

### Known limits
- Extracted and split files contain the pages and their annotations, but not the document's
  bookmarks or form structure.
- Pages of password-protected PDFs can't be reordered yet (lopdf limitation).

## Polish (Phase 5)
Code: `src-tauri/src/docops.rs`, `src-tauri/src/redact.rs`, `.github/workflows/build.yml`.

- **Export as images:** PNG or JPEG at 72/150/300 dpi.
- **Properties:** file details; title/author/subject/keywords written to the Info dictionary.
- **Passwords:** AES-256 (PDF 2.0, revision 6) with print/copy/edit permissions. Protected PDFs are
  decrypted in memory when opened, so every feature works on them, and encrypted again on save.
- **Reduce file size:** opaque over-resolution images are resampled to JPEG at 220/150/96 dpi, then
  unused objects are removed and the file is repacked with compressed object streams. Undoable.
- **Redaction:** mark areas in Edit mode, then apply. Text under an area is removed (partly
  covered text objects are rebuilt from their other characters), covered image pixels are blacked
  out in the image data, shapes entirely inside are removed, touching annotations and form
  widgets are deleted, and black boxes are drawn.
- **Builds:** GitHub Actions builds Windows, macOS (Apple silicon and Intel) and Linux installers
  and runs the engine tests on each. A `v*` tag creates a draft release.
- **Auto-update:** release builds check GitHub for `latest.json` and offer "Install and restart".
  Updates are signed; the public key is in `tauri.conf.json`, the private key must be stored as
  the `TAURI_SIGNING_PRIVATE_KEY` repository secret.

### Known limits
- macOS builds are not code-signed or notarized yet: first launch needs right-click → Open.
- Windows installers are not code-signed yet (SmartScreen warning).
- Redaction keeps vector shapes that are only partly under an area (they carry no text).
- Compression skips images with transparency.

## Performance targets
- Cold start < 1 s; first page < 300 ms; < 150 MB RAM with a 100-page document open.
- Only pages near the viewport are rendered; canvases far from view are released.

## Known risks
- Subset fonts → handled by font substitution (see above).
- XFA forms → PDFium has partial support; show them read-only and say so.
- Saving → write to a temp file and swap atomically; keep a backup of the original.
