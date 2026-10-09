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

/** One entry per character. Boxes are [left, top, right, bottom] * n, in page points, top-left origin. */
export interface PageText {
  codes: number[];
  boxes: number[];
}

export interface Link {
  rect: [number, number, number, number];
  page: number | null;
  uri: string | null;
}

export interface OutlineItem {
  title: string;
  page: number | null;
  children: OutlineItem[];
}

export interface SearchHit {
  page: number;
  start: number;
  len: number;
}

/** Error string returned by the backend when a password is needed or wrong. */
export const PASSWORD_REQUIRED = "PASSWORD_REQUIRED";

export function openDocument(path: string, password?: string): Promise<DocInfo> {
  return invoke<DocInfo>("open_document", { path, password: password ?? null });
}

export function closeDocument(id: number): Promise<void> {
  textCache.delete(id);
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

const textCache = new Map<number, Map<number, Promise<PageText>>>();

/** Page text with character boxes; cached per document. */
export function pageText(id: number, page: number): Promise<PageText> {
  let doc = textCache.get(id);
  if (!doc) textCache.set(id, (doc = new Map()));
  let t = doc.get(page);
  if (!t) {
    t = invoke<PageText>("page_text", { id, page });
    t.catch(() => doc.delete(page));
    doc.set(page, t);
  }
  return t;
}

export function pageLinks(id: number, page: number): Promise<Link[]> {
  return invoke<Link[]>("page_links", { id, page });
}

export function outline(id: number): Promise<OutlineItem[]> {
  return invoke<OutlineItem[]>("outline", { id });
}

export function search(id: number, query: string, matchCase: boolean): Promise<SearchHit[]> {
  return invoke<SearchHit[]>("search", { id, query, matchCase });
}

export function takeStartupFiles(): Promise<string[]> {
  return invoke<string[]>("take_startup_files");
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

export function textOf(t: PageText, from = 0, to = t.codes.length): string {
  let s = "";
  for (let i = from; i < to; i++) s += String.fromCodePoint(t.codes[i] || 0xfffd);
  return s;
}

// ---- Recent files (per-machine convenience, so localStorage is enough) ----

const RECENT_KEY = "recent-files";
const RECENT_MAX = 12;

export function recentFiles(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((p) => typeof p === "string") : [];
  } catch {
    return [];
  }
}

export function rememberRecent(path: string, remove = false): string[] {
  const list = recentFiles().filter((p) => p.toLowerCase() !== path.toLowerCase());
  if (!remove) list.unshift(path);
  const next = list.slice(0, RECENT_MAX);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    /* storage unavailable: recent list just won't persist */
  }
  return next;
}
