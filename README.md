# PDF Editor

A lightweight, open-source PDF viewer and editor for Windows (macOS and Linux later).
It's built with Tauri 2, Svelte 5 and PDFium. See [docs/ROADMAP.md](docs/ROADMAP.md) for the plan.

## Prerequisites (Windows)
- [Rust](https://rustup.rs) (stable, MSVC toolchain) and Visual Studio C++ Build Tools
- Node.js 20+ and pnpm
- WebView2 (preinstalled on Windows 10/11)

## Setup
```powershell
pnpm install
./scripts/fetch-pdfium.ps1   # downloads pdfium.dll into src-tauri/pdfium/
```

## Run / build
```powershell
pnpm tauri dev      # development
pnpm tauri build    # release installers (NSIS + MSI) in src-tauri/target/release/bundle
```

## Layout
- `src/` — Svelte UI (`lib/PdfView.svelte` is the virtualized page view, `lib/api.ts` wraps the IPC calls)
- `src-tauri/src/engine.rs` — PDFium worker thread (open, render, and later edit)
- `src-tauri/src/lib.rs` — Tauri commands

## License
MIT. PDFium is BSD-3-Clause / Apache-2.0.
