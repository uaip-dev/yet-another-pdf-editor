import { invoke } from "@tauri-apps/api/core";
import type { DocState, PageInfo } from "./api";

/**
 * What extension UI (the `$pro` slot, see docs/EXTENSIONS.md) can see and do.
 * Community builds ship empty slot components; other builds point the `$pro`
 * alias at their own components via the YAPE_PRO_UI environment variable.
 */
export interface ExtensionContext {
  /** The active document, or null on the start screen. */
  doc: {
    id: number;
    path: string;
    pageCount: number;
    currentPage: number;
    /** Selected pages in the thumbnail panel (or the current page). */
    pages: number[];
    /** Size of every page in points, as displayed (after rotation). */
    sizes: PageInfo[];
  } | null;
  /** Feature names registered by backend extensions. */
  capabilities: string[];
  /** Apply the result of a document change (refreshes pages, undo, dirty state). */
  onstate: (s: DocState) => void;
  onnotice: (message: string) => void;
  onerror: (message: string) => void;
  /** Re-reads capabilities (e.g. after a license is activated) so the UI updates. */
  reloadCapabilities: () => Promise<void>;
}

export function capabilities(): Promise<string[]> {
  return invoke<string[]>("capabilities").catch(() => []);
}
