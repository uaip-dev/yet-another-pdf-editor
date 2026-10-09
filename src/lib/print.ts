import { renderPage, type DocInfo } from "./api";

const PRINT_DPI = 150;

/**
 * Prints a document by rasterising its pages into a print-only container
 * and opening the system print dialog. `root` is hidden on screen and is the
 * only thing shown under `@media print`.
 */
export async function printDocument(
  doc: DocInfo,
  root: HTMLElement,
  onProgress: (done: number, total: number) => void,
  signal: AbortSignal,
): Promise<void> {
  const urls: string[] = [];
  try {
    for (let i = 0; i < doc.pages.length; i++) {
      if (signal.aborted) return;
      onProgress(i, doc.pages.length);
      const p = doc.pages[i];
      const bmp = await renderPage(doc.id, i, Math.round((p.width / 72) * PRINT_DPI));
      const canvas = new OffscreenCanvas(bmp.width, bmp.height);
      canvas.getContext("2d")!.drawImage(bmp, 0, 0);
      bmp.close();
      const blob = await canvas.convertToBlob({ type: "image/jpeg", quality: 0.92 });
      urls.push(URL.createObjectURL(blob));
    }
    onProgress(doc.pages.length, doc.pages.length);

    root.replaceChildren(
      ...urls.map((src, i) => {
        const img = new Image();
        img.src = src;
        img.className = doc.pages[i].width > doc.pages[i].height ? "landscape" : "portrait";
        return img;
      }),
    );
    await Promise.all([...root.querySelectorAll("img")].map((img) => img.decode().catch(() => {})));
    // print() may or may not block depending on the webview; wait for
    // afterprint either way before tearing down the images.
    const closed = new Promise((r) => window.addEventListener("afterprint", r, { once: true }));
    window.print();
    await closed;
  } finally {
    root.replaceChildren();
    urls.forEach(URL.revokeObjectURL);
  }
}
