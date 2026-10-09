# Downloads the prebuilt PDFium shared library (bblanchon/pdfium-binaries, BSD/Apache)
# into src-tauri/pdfium/. Pin $Tag for reproducible builds.
param(
  [string]$Tag = "chromium/8086",
  [string]$Platform = "win-x64"
)
$ErrorActionPreference = "Stop"
$dest = Join-Path $PSScriptRoot "..\src-tauri\pdfium"
$url = if ($Tag -eq "latest") {
  "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-$Platform.tgz"
} else {
  "https://github.com/bblanchon/pdfium-binaries/releases/download/$Tag/pdfium-$Platform.tgz"
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("pdfium-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp, $dest | Out-Null
try {
  Write-Host "Downloading $url"
  Invoke-WebRequest $url -OutFile "$tmp\pdfium.tgz"
  tar -xzf "$tmp\pdfium.tgz" -C $tmp
  Copy-Item "$tmp\bin\pdfium.dll" $dest -Force
  Copy-Item "$tmp\LICENSE" "$dest\LICENSE.pdfium" -Force -ErrorAction SilentlyContinue
  if (Test-Path "$tmp\VERSION") { Copy-Item "$tmp\VERSION" "$dest\VERSION" -Force; Get-Content "$tmp\VERSION" }
  Write-Host "PDFium installed to $dest"
} finally {
  Remove-Item $tmp -Recurse -Force
}
