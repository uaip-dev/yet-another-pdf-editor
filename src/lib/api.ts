import { invoke } from "@tauri-apps/api/core";

/** Page size in PDF points (1/72 inch). */
export interface PageInfo {
  width: number;
  height: number;
}

export interface DocInfo {
  id: number;
  path: string;
  title: string | null;
  pages: PageInfo[];
}

/** Error string returned by the backend when a password is needed or wrong. */
export const PASSWORD_REQUIRED = "PASSWORD_REQUIRED";

export function openDocument(path: string, password?: string): Promise<DocInfo> {
  return invoke<DocInfo>("open_document", { path, password: password ?? null });
}

export function closeDocument(id: number): Promise<void> {
  return invoke("close_document", { id });
}

/** Renders a page at the given pixel width and returns it as an ImageBitmap. */
export async function renderPage(id: number, page: number, width: number): Promise<ImageBitmap> {
  const buf = await invoke<ArrayBuffer>("render_page", { id, page, width });
  const header = new DataView(buf, 0, 8);
  const w = header.getUint32(0, true);
  const h = header.getUint32(4, true);
  const pixels = new Uint8ClampedArray(buf, 8, w * h * 4);
  return createImageBitmap(new ImageData(pixels, w, h));
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
