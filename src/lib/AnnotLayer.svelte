<script lang="ts" module>
  export type AnnotTool =
    | "select"
    | "highlight"
    | "underline"
    | "strikeout"
    | "note"
    | "pen"
    | "rectangle"
    | "ellipse"
    | "signature";

  export const MARKUP_TOOLS = ["highlight", "underline", "strikeout"] as const;

  /** A signature ready to place: PNG bytes, or an image file. */
  export type Signature =
    | { kind: "png"; png: Uint8Array; aspect: number; url: string }
    | { kind: "file"; path: string; aspect: number; url: string };
</script>

<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import {
    addAnnotation,
    addImageSized,
    addSignature,
    changeAnnotation,
    type AnnotInfo,
    type DocState,
    type Rect4,
    type Rgb,
  } from "./api";

  interface Props {
    docId: number;
    page: number;
    width: number;
    height: number;
    scale: number;
    revision: number;
    annotations: AnnotInfo[];
    tool: AnnotTool;
    color: Rgb;
    strokeWidth: number;
    signature: Signature | null;
    selected: { page: number; id: number } | null;
    onselect: (sel: { page: number; id: number } | null) => void;
    run: (op: () => Promise<DocState>) => Promise<boolean>;
    ondone: () => void;
  }

  let {
    docId,
    page,
    width,
    height,
    scale,
    revision,
    annotations,
    tool,
    color,
    strokeWidth,
    signature,
    selected,
    onselect,
    run,
    ondone,
  }: Props = $props();

  const SIGNATURE_WIDTH = 150; // points

  let layer: HTMLDivElement;
  let draft: { id: number; rect: Rect4 } | null = $state(null);
  // Pen strokes not yet committed (grouped into one ink annotation).
  let strokes: [number, number][][] = $state([]);
  let penTimer: ReturnType<typeof setTimeout> | undefined;
  // Shape being dragged out.
  let shape: Rect4 | null = $state(null);
  // Note editor: a new note at a point, or an existing note.
  let noteEdit: { at: [number, number]; id: number | null; text: string } | null = $state(null);
  let noteBox: HTMLTextAreaElement | undefined = $state();
  let hover: [number, number] | null = $state(null);

  const pct = (r: Rect4) =>
    `left:${(r[0] / width) * 100}%;top:${(r[1] / height) * 100}%;` +
    `width:${((r[2] - r[0]) / width) * 100}%;height:${((r[3] - r[1]) / height) * 100}%`;
  const cssColor = $derived(`rgb(${color.join(",")})`);

  function pointOf(e: PointerEvent | MouseEvent): [number, number] {
    const r = layer.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * width, ((e.clientY - r.top) / r.height) * height];
  }

  const rectOf = (a: AnnotInfo): Rect4 => (draft && draft.id === a.id ? draft.rect : a.rect);

  // ---- Select / move ----

  function startMove(e: PointerEvent, a: AnnotInfo) {
    if (tool !== "select" || e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    onselect({ page, id: a.id });
    if (!a.movable) return;
    const start = a.rect;
    const [sx, sy] = [e.clientX, e.clientY];
    let moved = false;
    const move = (ev: PointerEvent) => {
      const dx = (ev.clientX - sx) / scale;
      const dy = (ev.clientY - sy) / scale;
      if (!moved && Math.hypot(ev.clientX - sx, ev.clientY - sy) < 3) return;
      moved = true;
      draft = { id: a.id, rect: [start[0] + dx, start[1] + dy, start[2] + dx, start[3] + dy] };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (moved && draft) {
        const rect = draft.rect;
        run(() => changeAnnotation(docId, page, revision, a.id, { type: "rect", value: rect })).finally(() => (draft = null));
      } else draft = null;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  export async function deleteSelected() {
    if (!selected || selected.page !== page) return;
    const id = selected.id;
    onselect(null);
    await run(() => changeAnnotation(docId, page, revision, id, { type: "delete" }));
  }

  async function openNote(a: AnnotInfo) {
    noteEdit = { at: [a.rect[0], a.rect[3]], id: a.id, text: a.contents };
    await tick();
    noteBox?.focus({ preventScroll: true });
  }

  export function editSelected() {
    const a = annotations.find((x) => selected?.page === page && x.id === selected.id);
    if (a?.kind === "note") openNote(a);
  }

  async function commitNote() {
    const n = noteEdit;
    if (!n) return;
    noteEdit = null;
    if (n.id === null) {
      if (n.text.trim()) await run(() => addAnnotation(docId, page, { type: "note", at: n.at, text: n.text, color }));
      ondone();
    } else {
      const a = annotations.find((x) => x.id === n.id);
      if (a && a.contents !== n.text) await run(() => changeAnnotation(docId, page, revision, n.id!, { type: "contents", value: n.text }));
    }
  }

  // ---- Drawing tools ----

  function flushPen() {
    clearTimeout(penTimer);
    // Take the strokes before sending so they can never be committed twice.
    const s = strokes.filter((x) => x.length).map((x) => [...x]);
    strokes = [];
    if (s.length) run(() => addAnnotation(docId, page, { type: "ink", strokes: s, color, width: strokeWidth }));
  }

  /** Commits pending pen strokes (e.g. when switching tools). */
  export function finish() {
    flushPen();
  }

  async function onDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    if (noteEdit) return commitNote();
    const p = pointOf(e);
    if (tool === "select") return onselect(null);
    if (tool === "note") {
      noteEdit = { at: p, id: null, text: "" };
      await tick();
      noteBox?.focus({ preventScroll: true });
      return;
    }
    if (tool === "signature" && signature) {
      const sig = signature;
      const at: [number, number] = [p[0] - SIGNATURE_WIDTH / 2, p[1] - SIGNATURE_WIDTH / sig.aspect / 2];
      await run(() =>
        sig.kind === "png"
          ? addSignature(docId, page, at[0], at[1], sig.png, SIGNATURE_WIDTH)
          : addImageSized(docId, page, at[0], at[1], sig.path, SIGNATURE_WIDTH),
      );
      ondone();
      return;
    }
    if (tool === "pen") {
      clearTimeout(penTimer);
      strokes = [...strokes, [p]];
      const move = (ev: PointerEvent) => {
        const q = pointOf(ev);
        const cur = strokes[strokes.length - 1];
        const last = cur[cur.length - 1];
        // Skip points closer than half a point to keep strokes light.
        if (Math.hypot(q[0] - last[0], q[1] - last[1]) * scale >= 1.5) {
          cur.push([Math.round(q[0] * 100) / 100, Math.round(q[1] * 100) / 100]);
        }
      };
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        // Strokes made in quick succession become one annotation (like handwriting).
        penTimer = setTimeout(flushPen, 900);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      return;
    }
    if (tool === "rectangle" || tool === "ellipse") {
      const kind = tool;
      shape = [p[0], p[1], p[0], p[1]];
      const move = (ev: PointerEvent) => {
        const q = pointOf(ev);
        shape = [Math.min(p[0], q[0]), Math.min(p[1], q[1]), Math.max(p[0], q[0]), Math.max(p[1], q[1])];
      };
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        const r = shape;
        shape = null;
        if (r && r[2] - r[0] > 3 && r[3] - r[1] > 3) {
          run(() => addAnnotation(docId, page, { type: "shape", kind, rect: r, color, width: strokeWidth }));
        }
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    }
  }

  // Pending pen strokes are committed when switching tool or leaving the page.
  $effect(() => {
    if (tool !== "pen" && strokes.length) flushPen();
  });
  onDestroy(flushPen);

  const strokePath = (s: [number, number][]) =>
    s.map(([x, y], i) => `${i ? "L" : "M"}${x} ${y}`).join(" ") + (s.length === 1 ? " l0.01 0" : "");
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={layer}
  class="layer tool-{tool}"
  onpointerdown={onDown}
  onpointermove={(e) => (hover = tool === "signature" ? pointOf(e) : null)}
  onpointerleave={() => (hover = null)}
>
  {#if tool === "select"}
    {#each annotations as a (a.id)}
      {@const sel = selected?.page === page && selected.id === a.id}
      <div
        class="box"
        class:selected={sel}
        class:movable={a.movable}
        style={pct(rectOf(a))}
        title={[a.kind, a.author, a.contents].filter(Boolean).join(" — ")}
        onpointerdown={(e) => startMove(e, a)}
        ondblclick={() => a.kind === "note" && openNote(a)}
      >
        {#if sel}
          <div class="mini-bar" onpointerdown={(e) => e.stopPropagation()}>
            {#if a.kind === "note"}
              <button onclick={() => openNote(a)}><Icon name="edit" size={15} />Edit note</button>
            {/if}
            <button onclick={deleteSelected} title="Delete (Del)"><Icon name="trash" size={15} /></button>
          </div>
        {/if}
      </div>
    {/each}
  {/if}

  <svg class="ink" viewBox="0 0 {width} {height}" preserveAspectRatio="none">
    {#each strokes as s}
      <path d={strokePath(s)} stroke={cssColor} stroke-width={strokeWidth} />
    {/each}
    {#if shape}
      {#if tool === "ellipse"}
        <ellipse
          cx={(shape[0] + shape[2]) / 2}
          cy={(shape[1] + shape[3]) / 2}
          rx={(shape[2] - shape[0]) / 2}
          ry={(shape[3] - shape[1]) / 2}
          stroke={cssColor}
          stroke-width={strokeWidth}
        />
      {:else}
        <rect x={shape[0]} y={shape[1]} width={shape[2] - shape[0]} height={shape[3] - shape[1]} stroke={cssColor} stroke-width={strokeWidth} />
      {/if}
    {/if}
  </svg>

  {#if tool === "signature" && signature && hover}
    <img
      class="ghost"
      src={signature.url}
      alt=""
      style="left:{((hover[0] - SIGNATURE_WIDTH / 2) / width) * 100}%;top:{((hover[1] - SIGNATURE_WIDTH / signature.aspect / 2) / height) * 100}%;width:{(SIGNATURE_WIDTH / width) * 100}%"
    />
  {/if}

  {#if noteEdit}
    <div class="note-pop" style="left:{(noteEdit.at[0] / width) * 100}%;top:{(noteEdit.at[1] / height) * 100}%">
      <textarea
        bind:this={noteBox}
        bind:value={noteEdit.text}
        placeholder="Write a note…"
        onpointerdown={(e) => e.stopPropagation()}
        onkeydown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") noteEdit = null;
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) commitNote();
        }}
        onblur={commitNote}
        aria-label="Note text"
      ></textarea>
      <span class="hint">Ctrl+Enter to save · Esc to cancel</span>
    </div>
  {/if}
</div>

<style>
  .layer {
    position: absolute;
    inset: 0;
  }
  .tool-note,
  .tool-signature {
    cursor: copy;
  }
  .tool-pen,
  .tool-rectangle,
  .tool-ellipse {
    cursor: crosshair;
  }
  .box {
    position: absolute;
    box-sizing: border-box;
    border: 1px dashed transparent;
    cursor: pointer;
  }
  .box.movable {
    cursor: move;
  }
  .box:hover {
    border-color: color-mix(in srgb, var(--accent) 70%, transparent);
  }
  .box.selected {
    border: 1.5px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .ink {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    overflow: visible;
  }
  .ink path,
  .ink rect,
  .ink ellipse {
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .ghost {
    position: absolute;
    opacity: 0.6;
    pointer-events: none;
  }
  .mini-bar {
    position: absolute;
    bottom: calc(100% + 6px);
    left: -2px;
    display: flex;
    gap: 2px;
    padding: 3px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    box-shadow: 0 2px 8px rgb(0 0 0 / 0.18);
    cursor: default;
    z-index: 2;
    white-space: nowrap;
  }
  .mini-bar button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-height: 26px;
    padding: 2px 8px;
    border: none;
    background: transparent;
    font-size: 12px;
  }
  .mini-bar button:hover {
    background: var(--btn-hover);
  }
  .note-pop {
    position: absolute;
    z-index: 3;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px;
    background: #fff8c4;
    border: 1px solid #d8c55a;
    border-radius: 6px;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.25);
  }
  .note-pop textarea {
    width: 220px;
    height: 90px;
    resize: both;
    border: none;
    background: transparent;
    color: #222;
    font: 13px system-ui, sans-serif;
    outline: none;
  }
  .note-pop .hint {
    font-size: 11px;
    color: #776a20;
  }
</style>
