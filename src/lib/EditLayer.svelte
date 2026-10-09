<script lang="ts" module>
  export type Tool = "edit" | "addText" | "addImage";

  export interface EditSelection {
    page: number;
    target: "block" | "image";
    id: number;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import {
    addImage,
    addText,
    deleteObject,
    editText,
    IMAGE_FILTERS,
    replaceImage,
    transformObject,
    type DocState,
    type NewTextStyle,
    type PageLayout,
    type Rect4,
    type TextBlock,
  } from "./api";

  interface Props {
    docId: number;
    page: number;
    /** Page size in points. */
    width: number;
    height: number;
    /** CSS px per point. */
    scale: number;
    layout: PageLayout;
    tool: Tool;
    selection: EditSelection | null;
    newTextStyle: NewTextStyle;
    onselect: (sel: EditSelection | null) => void;
    /** Runs a document change; the parent applies the resulting state. */
    run: (op: () => Promise<DocState>) => Promise<boolean>;
    ondone: () => void;
  }

  let { docId, page, width, height, scale, layout, tool, selection, newTextStyle, onselect, run, ondone }: Props = $props();

  // Rect currently being dragged/resized (page points), before it is committed.
  let draft: { target: "block" | "image"; id: number; rect: Rect4 } | null = $state(null);
  // Inline text editor: an existing block, or new text at a point.
  let editing: { block: TextBlock | null; at: [number, number]; text: string } | null = $state(null);
  let editor: HTMLTextAreaElement | undefined = $state();

  const pct = (r: Rect4) =>
    `left:${(r[0] / width) * 100}%;top:${(r[1] / height) * 100}%;` +
    `width:${((r[2] - r[0]) / width) * 100}%;height:${((r[3] - r[1]) / height) * 100}%`;

  const isSelected = (target: "block" | "image", id: number) =>
    selection?.page === page && selection.target === target && selection.id === id;

  function rectOf(target: "block" | "image", id: number): Rect4 {
    if (draft && draft.target === target && draft.id === id) return draft.rect;
    const item = target === "block" ? layout.blocks.find((b) => b.id === id) : layout.images.find((i) => i.id === id);
    return item!.rect;
  }

  /** Page point under a pointer event. */
  function pointOf(e: PointerEvent | MouseEvent, el: HTMLElement): [number, number] {
    const r = el.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * width, ((e.clientY - r.top) / r.height) * height];
  }

  // ---- Drag to move, handles to resize ----

  function startDrag(e: PointerEvent, target: "block" | "image", id: number, handle: number | null = null) {
    if (e.button !== 0 || editing) return;
    e.stopPropagation();
    e.preventDefault();
    onselect({ page, target, id });
    const start = rectOf(target, id);
    const [sx, sy] = [e.clientX, e.clientY];
    const aspect = (start[2] - start[0]) / (start[3] - start[1]);
    let moved = false;

    const move = (ev: PointerEvent) => {
      const dx = (ev.clientX - sx) / scale;
      const dy = (ev.clientY - sy) / scale;
      if (!moved && Math.hypot(ev.clientX - sx, ev.clientY - sy) < 3) return;
      moved = true;
      let r: Rect4 = [...start];
      if (handle === null) {
        r = [start[0] + dx, start[1] + dy, start[2] + dx, start[3] + dy];
      } else {
        // Corners: 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
        const left = handle === 0 || handle === 3;
        const top = handle === 0 || handle === 1;
        if (left) r[0] = Math.min(start[0] + dx, start[2] - 4);
        else r[2] = Math.max(start[2] + dx, start[0] + 4);
        if (top) r[1] = Math.min(start[1] + dy, start[3] - 4);
        else r[3] = Math.max(start[3] + dy, start[1] + 4);
        if (!ev.shiftKey) {
          // Keep the aspect ratio, driven by the width change.
          const h = (r[2] - r[0]) / aspect;
          if (top) r[1] = r[3] - h;
          else r[3] = r[1] + h;
        }
      }
      draft = { target, id, rect: r };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (moved && draft) {
        const rect = draft.rect;
        run(() => transformObject(docId, page, layout.revision, target, id, rect)).finally(() => (draft = null));
      } else {
        draft = null;
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // ---- Text editing ----

  async function startEdit(block: TextBlock) {
    onselect({ page, target: "block", id: block.id });
    editing = { block, at: [block.rect[0], block.rect[1]], text: block.text };
    await tick();
    editor?.focus({ preventScroll: true });
  }

  async function commitEdit() {
    const ed = editing;
    if (!ed) return;
    editing = null;
    if (ed.block) {
      if (ed.text !== ed.block.text) {
        const id = ed.block.id;
        await run(() => editText(docId, page, layout.revision, id, ed.text));
      }
    } else if (ed.text.trim()) {
      await run(() => addText(docId, page, ed.at[0], ed.at[1], ed.text, newTextStyle));
      ondone();
    } else {
      ondone();
    }
  }

  function onEditorKey(e: KeyboardEvent) {
    e.stopPropagation(); // keep app shortcuts (Ctrl+A, Delete...) inside the editor
    if (e.key === "Escape") {
      const wasNew = !editing?.block;
      editing = null;
      if (wasNew) ondone();
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      commitEdit();
    }
  }

  // ---- Layer clicks (add text / add image / deselect) ----

  async function onLayerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    if (editing) return commitEdit();
    const at = pointOf(e, e.currentTarget as HTMLElement);
    if (tool === "addText") {
      editing = { block: null, at, text: "" };
      await tick();
      editor?.focus({ preventScroll: true });
    } else if (tool === "addImage") {
      const path = await open({ multiple: false, directory: false, filters: IMAGE_FILTERS });
      if (path) await run(() => addImage(docId, page, at[0], at[1], path));
      ondone();
    } else {
      onselect(null);
    }
  }

  export async function replaceSelected() {
    if (!selection || selection.page !== page || selection.target !== "image") return;
    const id = selection.id;
    const path = await open({ multiple: false, directory: false, filters: IMAGE_FILTERS });
    if (path) await run(() => replaceImage(docId, page, layout.revision, id, path));
  }

  export async function deleteSelected() {
    if (!selection || selection.page !== page) return;
    const { target, id } = selection;
    onselect(null);
    await run(() => deleteObject(docId, page, layout.revision, target, id));
  }

  export function editSelected() {
    if (selection?.page !== page || selection.target !== "block") return;
    const b = layout.blocks.find((x) => x.id === selection!.id);
    if (b) startEdit(b);
  }

  /** Arrow-key nudge of the selected object, in points. */
  export function nudge(dx: number, dy: number) {
    if (!selection || selection.page !== page) return;
    const { target, id } = selection;
    const r = rectOf(target, id);
    run(() => transformObject(docId, page, layout.revision, target, id, [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]));
  }

  const editorStyle = $derived.by(() => {
    if (!editing) return "";
    const b = editing.block;
    const size = (b?.size ?? newTextStyle.size) * scale;
    const family = b?.family ?? newTextStyle.family;
    const bold = b?.bold ?? newTextStyle.bold;
    const italic = b?.italic ?? newTextStyle.italic;
    const color = b?.color ?? `rgb(${newTextStyle.color.join(",")})`;
    const lh = b ? Math.max(b.lineHeight, b.size) * scale : size * 1.25;
    const multiLine = b ? b.rect[3] - b.rect[1] > b.lineHeight * 1.5 : false;
    return (
      `left:${(editing.at[0] / width) * 100}%;top:${(editing.at[1] / height) * 100}%;` +
      `font-size:${size}px;line-height:${lh}px;font-family:${family};` +
      `font-weight:${bold ? 700 : 400};font-style:${italic ? "italic" : "normal"};color:${color};` +
      (multiLine && b ? `width:${((b.rect[2] - b.rect[0]) / width) * 100 + 1}%;` : "min-width:2em;white-space:pre;")
    );
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="layer"
  class:add-text={tool === "addText"}
  class:add-image={tool === "addImage"}
  onpointerdown={onLayerDown}
>
  {#each layout.images as im (im.id)}
    {@const sel = isSelected("image", im.id)}
    <div
      class="box image"
      class:selected={sel}
      style={pct(rectOf("image", im.id))}
      onpointerdown={(e) => startDrag(e, "image", im.id)}
      title="Image {im.width}×{im.height}px — drag to move"
    >
      {#if sel}
        {#each [0, 1, 2, 3] as h}
          <span class="handle h{h}" onpointerdown={(e) => startDrag(e, "image", im.id, h)}></span>
        {/each}
        <div class="mini-bar" onpointerdown={(e) => e.stopPropagation()}>
          <button onclick={replaceSelected} title="Replace image"><Icon name="replace" size={15} />Replace</button>
          <button onclick={deleteSelected} title="Delete (Del)"><Icon name="trash" size={15} /></button>
        </div>
      {/if}
    </div>
  {/each}

  {#each layout.blocks as b (b.id)}
    {@const sel = isSelected("block", b.id)}
    {#if editing?.block?.id !== b.id}
      <div
        class="box block"
        class:selected={sel}
        style={pct(rectOf("block", b.id))}
        onpointerdown={(e) => startDrag(e, "block", b.id)}
        ondblclick={() => startEdit(b)}
        title="Double-click to edit, drag to move"
      >
        {#if sel}
          <div class="mini-bar" onpointerdown={(e) => e.stopPropagation()}>
            <button onclick={() => startEdit(b)} title="Edit text (Enter)"><Icon name="edit" size={15} />Edit</button>
            <button onclick={deleteSelected} title="Delete (Del)"><Icon name="trash" size={15} /></button>
          </div>
        {/if}
      </div>
    {/if}
  {/each}

  {#if editing}
    <textarea
      bind:this={editor}
      bind:value={editing.text}
      class="editor"
      style={editorStyle}
      spellcheck="false"
      onkeydown={onEditorKey}
      onblur={commitEdit}
      onpointerdown={(e) => e.stopPropagation()}
      aria-label="Edit text"
    ></textarea>
  {/if}
</div>

<style>
  .layer {
    position: absolute;
    inset: 0;
    cursor: default;
  }
  .layer.add-text {
    cursor: text;
  }
  .layer.add-image {
    cursor: copy;
  }
  .box {
    position: absolute;
    box-sizing: border-box;
    border: 1px dashed transparent;
    cursor: move;
  }
  .layer:not(.add-text):not(.add-image) .box:hover {
    border-color: color-mix(in srgb, var(--accent) 70%, transparent);
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }
  .add-text .box,
  .add-image .box {
    pointer-events: none;
  }
  .box.block {
    margin: -2px;
    padding: 2px;
    box-sizing: content-box;
  }
  .box.selected {
    border: 1.5px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .handle {
    position: absolute;
    width: 10px;
    height: 10px;
    background: #fff;
    border: 1.5px solid var(--accent);
    border-radius: 2px;
  }
  .h0 {
    left: -6px;
    top: -6px;
    cursor: nwse-resize;
  }
  .h1 {
    right: -6px;
    top: -6px;
    cursor: nesw-resize;
  }
  .h2 {
    right: -6px;
    bottom: -6px;
    cursor: nwse-resize;
  }
  .h3 {
    left: -6px;
    bottom: -6px;
    cursor: nesw-resize;
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
  .editor {
    position: absolute;
    margin: -3px 0 0 -3px;
    padding: 2px;
    border: 1.5px solid var(--accent);
    border-radius: 2px;
    background: #fff;
    outline: none;
    resize: none;
    overflow: hidden;
    field-sizing: content;
    z-index: 3;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.2);
  }
</style>
