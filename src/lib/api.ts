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

// ---- Editing ----

/** Rects are [left, top, right, bottom] in page points (top-left origin). */
export type Rect4 = [number, number, number, number];

export interface TextBlock {
  id: number;
  rect: Rect4;
  text: string;
  size: number;
  lineHeight: number;
  family: "serif" | "sans-serif" | "monospace";
  bold: boolean;
  italic: boolean;
  color: string;
}

export interface ImageBox {
  id: number;
  rect: Rect4;
  width: number;
  height: number;
}

export interface PageLayout {
  revision: number;
  blocks: TextBlock[];
  images: ImageBox[];
}

export interface DocState {
  revision: number;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
  /** Characters drawn with a substitute font by the last edit. */
  substituted: string;
  path: string;
  /** The page list, after operations that may change it. */
  pages: PageInfo[] | null;
}

export type Target = "block" | "image";

/** Error returned when an edit names an out-of-date revision. */
export const STALE = "STALE";

export function pageLayout(id: number, page: number): Promise<PageLayout> {
  return invoke<PageLayout>("page_layout", { id, page });
}

export function docState(id: number): Promise<DocState> {
  return invoke<DocState>("doc_state", { id });
}

export function editText(id: number, page: number, revision: number, block: number, text: string): Promise<DocState> {
  return invoke<DocState>("edit_text", { id, page, revision, block, text });
}

export interface NewTextStyle {
  size: number;
  family: "serif" | "sans-serif" | "monospace";
  bold: boolean;
  italic: boolean;
  color: [number, number, number];
}

export function addText(id: number, page: number, x: number, y: number, text: string, style: NewTextStyle): Promise<DocState> {
  return invoke<DocState>("add_text", { id, page, x, y, text, ...style });
}

export function transformObject(
  id: number,
  page: number,
  revision: number,
  target: Target,
  index: number,
  rect: Rect4,
): Promise<DocState> {
  return invoke<DocState>("transform_object", { id, page, revision, target, index, rect });
}

export function deleteObject(id: number, page: number, revision: number, target: Target, index: number): Promise<DocState> {
  return invoke<DocState>("delete_object", { id, page, revision, target, index });
}

export function replaceImage(id: number, page: number, revision: number, index: number, path: string): Promise<DocState> {
  return invoke<DocState>("replace_image", { id, page, revision, index, path });
}

export function addImage(id: number, page: number, x: number, y: number, path: string): Promise<DocState> {
  return invoke<DocState>("add_image", { id, page, x, y, path, width: null });
}

/** Adds an image file scaled to `width` points (aspect kept). */
export function addImageSized(id: number, page: number, x: number, y: number, path: string, width: number): Promise<DocState> {
  return invoke<DocState>("add_image", { id, page, x, y, path, width });
}

export function undo(id: number): Promise<DocState> {
  return invoke<DocState>("undo", { id });
}

export function redo(id: number): Promise<DocState> {
  return invoke<DocState>("redo", { id });
}

export function saveDocument(id: number, path?: string): Promise<DocState> {
  return invoke<DocState>("save_document", { id, path: path ?? null });
}

/** Drops cached page text after the document changed. */
export function invalidateText(id: number) {
  textCache.delete(id);
}

export const IMAGE_FILTERS = [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "bmp", "webp"] }];

// ---- Annotations, forms, signatures ----

export interface AnnotInfo {
  id: number;
  kind: string;
  rect: Rect4;
  color: string;
  contents: string;
  author: string;
  movable: boolean;
}

export interface FieldInfo {
  id: number;
  kind: "text" | "checkbox" | "radio" | "combo" | "list" | "button" | "signature" | "unknown";
  name: string;
  value: string;
  checked: boolean;
  options: string[];
  selected: number | null;
  rect: Rect4;
  readOnly: boolean;
  multiline: boolean;
  password: boolean;
}

export interface PageExtras {
  revision: number;
  annotations: AnnotInfo[];
  fields: FieldInfo[];
}

export type Rgb = [number, number, number];
export type MarkupKind = "highlight" | "underline" | "strikeout";

export type NewAnnot =
  | { type: "markup"; kind: MarkupKind; rects: Rect4[]; color: Rgb }
  | { type: "note"; at: [number, number]; text: string; color: Rgb }
  | { type: "ink"; strokes: [number, number][][]; color: Rgb; width: number }
  | { type: "shape"; kind: "rectangle" | "ellipse"; rect: Rect4; color: Rgb; width: number };

export type AnnotChange = { type: "rect"; value: Rect4 } | { type: "contents"; value: string } | { type: "delete" };

export type FieldValue = { type: "text"; value: string } | { type: "checked"; value: boolean } | { type: "select"; value: number };

export function pageExtras(id: number, page: number): Promise<PageExtras> {
  return invoke<PageExtras>("page_extras", { id, page });
}

export function addAnnotation(id: number, page: number, annot: NewAnnot): Promise<DocState> {
  return invoke<DocState>("add_annotation", { id, page, annot });
}

export function changeAnnotation(id: number, page: number, revision: number, annot: number, change: AnnotChange): Promise<DocState> {
  return invoke<DocState>("change_annotation", { id, page, revision, annot, change });
}

export function fillFields(id: number, page: number, revision: number, changes: [number, FieldValue][]): Promise<DocState> {
  return invoke<DocState>("fill_fields", { id, page, revision, changes });
}

export function addSignature(id: number, page: number, x: number, y: number, png: Uint8Array, width: number): Promise<DocState> {
  return invoke<DocState>("add_signature", { id, page, x, y, png: Array.from(png), width });
}

export function hexToRgb(hex: string): Rgb {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export async function readImageFile(path: string): Promise<Blob> {
  const buf = await invoke<ArrayBuffer>("read_image_file", { path });
  return new Blob([buf]);
}

// ---- Pages ----

export function rotatePages(id: number, pages: number[], delta: number): Promise<DocState> {
  return invoke<DocState>("rotate_pages", { id, pages, delta });
}

export function deletePages(id: number, pages: number[]): Promise<DocState> {
  return invoke<DocState>("delete_pages", { id, pages });
}

/** Moves `pages` so they sit before original index `before` (page count = end). */
export function movePages(id: number, pages: number[], before: number): Promise<DocState> {
  return invoke<DocState>("move_pages", { id, pages, before });
}

export function insertBlankPage(id: number, at: number, width: number, height: number): Promise<DocState> {
  return invoke<DocState>("insert_blank_page", { id, at, width, height });
}

export function insertPagesFromFile(id: number, at: number, path: string, password?: string): Promise<DocState> {
  return invoke<DocState>("insert_pages_from_file", { id, at, path, password: password ?? null });
}

export function insertImagePage(id: number, at: number, path: string): Promise<DocState> {
  return invoke<DocState>("insert_image_page", { id, at, path });
}

export function extractPages(id: number, pages: number[], out: string): Promise<void> {
  return invoke("extract_pages", { id, pages, out });
}

export function splitDocument(id: number, every: number, dir: string): Promise<string[]> {
  return invoke<string[]>("split_document", { id, every, dir });
}

/** "1-3, 5" style label for 0-based page indices. */
export function pageLabel(pages: number[]): string {
  const s = [...pages].sort((a, b) => a - b);
  const parts: string[] = [];
  for (let i = 0; i < s.length; i++) {
    let j = i;
    while (j + 1 < s.length && s[j + 1] === s[j] + 1) j++;
    parts.push(i === j ? `${s[i] + 1}` : `${s[i] + 1}-${s[j] + 1}`);
    i = j;
  }
  return parts.join(", ");
}
