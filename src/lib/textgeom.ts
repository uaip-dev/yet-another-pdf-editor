import type { PageText } from "./api";

export type Rect = [number, number, number, number]; // left, top, right, bottom

/** A caret position: between characters, 0..codes.length. */
export interface Caret {
  page: number;
  index: number;
}

export interface Selection {
  anchor: Caret;
  focus: Caret;
}

export function box(t: PageText, i: number): Rect | null {
  const b = t.boxes;
  const r: Rect = [b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]];
  return r[2] > r[0] || r[3] > r[1] ? r : null;
}

/** Caret index closest to a point (page points, top-left origin). */
export function hitTest(t: PageText, x: number, y: number): number {
  let best = -1;
  let bestScore = Infinity;
  for (let i = 0; i < t.codes.length; i++) {
    const r = box(t, i);
    if (!r) continue;
    const dx = x < r[0] ? r[0] - x : x > r[2] ? x - r[2] : 0;
    const dy = y < r[1] ? r[1] - y : y > r[3] ? y - r[3] : 0;
    // Being on the right line matters far more than horizontal distance.
    const score = dy * 20 + dx;
    if (score < bestScore) {
      bestScore = score;
      best = i;
      if (score === 0) break;
    }
  }
  if (best < 0) return 0;
  const r = box(t, best)!;
  return x > (r[0] + r[2]) / 2 ? best + 1 : best;
}

const isSpace = (c: number) => c === 32 || c === 9 || c === 10 || c === 13 || c === 0xa0;

/** [start, end) of the word around a caret index. */
export function wordAt(t: PageText, index: number): [number, number] {
  let s = Math.min(index, t.codes.length - 1);
  if (s < 0) return [0, 0];
  if (isSpace(t.codes[s]) && s > 0 && !isSpace(t.codes[s - 1])) s--;
  let e = s;
  while (s > 0 && !isSpace(t.codes[s - 1])) s--;
  while (e < t.codes.length && !isSpace(t.codes[e])) e++;
  return [s, e];
}

/** [start, end) of the line around a caret index (split on generated line breaks). */
export function lineAt(t: PageText, index: number): [number, number] {
  const nl = (c: number) => c === 10 || c === 13;
  let s = Math.min(index, t.codes.length);
  let e = s;
  while (s > 0 && !nl(t.codes[s - 1])) s--;
  while (e < t.codes.length && !nl(t.codes[e])) e++;
  return [s, e];
}

/** Highlight rectangles for characters [from, to), merged per line. */
export function rangeRects(t: PageText, from: number, to: number): Rect[] {
  const out: Rect[] = [];
  let cur: Rect | null = null;
  for (let i = from; i < to; i++) {
    const r = box(t, i);
    if (!r) continue;
    const sameLine =
      cur &&
      Math.abs(r[1] - cur[1]) < (cur[3] - cur[1]) * 0.5 &&
      r[0] >= cur[0] - 1 &&
      r[0] - cur[2] < (cur[3] - cur[1]) * 2;
    if (cur && sameLine) {
      cur[1] = Math.min(cur[1], r[1]);
      cur[2] = Math.max(cur[2], r[2]);
      cur[3] = Math.max(cur[3], r[3]);
    } else {
      if (cur) out.push(cur);
      cur = [...r];
    }
  }
  if (cur) out.push(cur);
  return out;
}

export function compareCaret(a: Caret, b: Caret): number {
  return a.page - b.page || a.index - b.index;
}

/** Normalised [start, end] of a selection. */
export function ordered(sel: Selection): [Caret, Caret] {
  return compareCaret(sel.anchor, sel.focus) <= 0
    ? [sel.anchor, sel.focus]
    : [sel.focus, sel.anchor];
}

/** Character range [from, to) of `page` covered by the selection, or null. */
export function selectionOnPage(sel: Selection, page: number, len: number): [number, number] | null {
  const [a, b] = ordered(sel);
  if (page < a.page || page > b.page) return null;
  const from = page === a.page ? a.index : 0;
  const to = page === b.page ? b.index : len;
  return to > from ? [from, to] : null;
}
