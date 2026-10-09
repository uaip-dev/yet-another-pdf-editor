<script lang="ts">
  import { onMount } from "svelte";
  import { renderPage, type DocInfo } from "./api";

  interface Props {
    doc: DocInfo;
    currentPage: number;
    /** Document revision; thumbnails are redrawn when it changes. */
    revision?: number;
    /** Selected page indices (for page operations). */
    selected: number[];
    onselect: (page: number) => void;
    onselectionchange: (pages: number[]) => void;
    /** Drag and drop: move `pages` before original index `before`. */
    onmove: (pages: number[], before: number) => void;
    ondelete: (pages: number[]) => void;
  }

  let { doc, currentPage, revision = 0, selected, onselect, onselectionchange, onmove, ondelete }: Props = $props();

  const THUMB_W = 120;

  let scroller: HTMLDivElement;
  let items: HTMLButtonElement[] = $state([]);
  let canvases: HTMLCanvasElement[] = $state([]);
  const drawn = new Set<number>();
  const visible = new Set<number>();
  let anchor = 0;
  // Drag and drop
  let dragging: number[] | null = $state(null);
  let dropBefore: number | null = $state(null);

  async function draw(i: number, force = false) {
    if (drawn.has(i) && !force) return;
    drawn.add(i);
    try {
      const bmp = await renderPage(doc.id, i, Math.round(THUMB_W * devicePixelRatio));
      if (!drawn.has(i)) return bmp.close();
      const c = canvases[i];
      c.width = bmp.width;
      c.height = bmp.height;
      c.getContext("2d")!.drawImage(bmp, 0, 0);
      bmp.close();
    } catch (e) {
      drawn.delete(i);
      console.error(e);
    }
  }

  let seenRevision = -1;
  $effect(() => {
    const r = revision;
    if (seenRevision !== -1 && r !== seenRevision) visible.forEach((i) => draw(i, true));
    seenRevision = r;
  });

  // Keep the current page's thumbnail in view. (scrollIntoView would also
  // scroll the window itself, shifting the whole app.)
  $effect(() => {
    const el = items[currentPage];
    if (!el || !scroller) return;
    const top = el.offsetTop; // .thumbs is the offset parent
    if (top < scroller.scrollTop) scroller.scrollTop = top - 8;
    else if (top + el.offsetHeight > scroller.scrollTop + scroller.clientHeight)
      scroller.scrollTop = top + el.offsetHeight - scroller.clientHeight + 8;
  });

  function onClick(e: MouseEvent, i: number) {
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    if (e.shiftKey) {
      const [a, b] = [Math.min(anchor, i), Math.max(anchor, i)];
      onselectionchange(Array.from({ length: b - a + 1 }, (_, k) => a + k));
    } else if (e.ctrlKey || e.metaKey) {
      onselectionchange(selected.includes(i) ? selected.filter((p) => p !== i) : [...selected, i].sort((x, y) => x - y));
      anchor = i;
    } else {
      onselectionchange([i]);
      anchor = i;
      onselect(i);
    }
  }

  function onKey(e: KeyboardEvent) {
    if ((e.key === "Delete" || e.key === "Backspace") && selected.length) {
      e.preventDefault();
      e.stopPropagation();
      ondelete(selected);
    } else if (e.key === "a" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      e.stopPropagation();
      onselectionchange(doc.pages.map((_, i) => i));
    }
  }

  // Reordering uses pointer events: on Windows, Tauri's file drop handling
  // disables HTML5 drag and drop inside the page.
  let suppressClick = false;

  function onPointerDown(e: PointerEvent, i: number) {
    if (e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey) return;
    const [sx, sy] = [e.clientX, e.clientY];
    let started = false;
    let scrollTimer: ReturnType<typeof setInterval> | undefined;
    let lastY = sy;
    const target = (x: number, y: number) => {
      const el = (document.elementFromPoint(x, y) as HTMLElement | null)?.closest<HTMLElement>(".thumb");
      if (!el || !scroller.contains(el)) return;
      const k = Number(el.dataset.index);
      const r = el.getBoundingClientRect();
      dropBefore = y < r.top + r.height / 2 ? k : k + 1;
    };
    const move = (ev: PointerEvent) => {
      if (!started) {
        if (Math.hypot(ev.clientX - sx, ev.clientY - sy) < 6) return;
        started = true;
        const pages = selected.includes(i) ? selected : [i];
        if (!selected.includes(i)) onselectionchange([i]);
        dragging = pages;
        // Auto-scroll while dragging near the panel's top or bottom edge.
        scrollTimer = setInterval(() => {
          const r = scroller.getBoundingClientRect();
          if (lastY < r.top + 30) scroller.scrollTop -= 14;
          else if (lastY > r.bottom - 30) scroller.scrollTop += 14;
        }, 30);
      }
      lastY = ev.clientY;
      target(ev.clientX, ev.clientY);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      clearInterval(scrollTimer);
      if (started) {
        suppressClick = true;
        finishDrag();
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function finishDrag() {
    const pages = dragging;
    const before = dropBefore;
    dragging = null;
    dropBefore = null;
    if (!pages || before === null) return;
    // Dropping a block right where it already is changes nothing.
    const sorted = [...pages].sort((a, b) => a - b);
    const contiguous = sorted.every((p, k) => k === 0 || p === sorted[k - 1] + 1);
    if (contiguous && before >= sorted[0] && before <= sorted[sorted.length - 1] + 1) return;
    onmove(sorted, before);
  }

  onMount(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const i = Number((entry.target as HTMLElement).dataset.index);
          if (entry.isIntersecting) {
            visible.add(i);
            draw(i);
          } else {
            visible.delete(i);
            if (drawn.delete(i) && canvases[i]) canvases[i].width = canvases[i].height = 0;
          }
        }
      },
      { root: scroller, rootMargin: "300px 0px" },
    );
    items.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="thumbs" class:reordering={!!dragging} bind:this={scroller} onkeydown={onKey}>
  {#each doc.pages as page, i (i)}
    <button
      class="thumb"
      class:current={i === currentPage}
      class:selected={selected.includes(i)}
      class:drop-before={dropBefore === i}
      class:drop-after={dropBefore === i + 1 && i === doc.pages.length - 1}
      class:dragged={dragging?.includes(i)}
      data-index={i}
      bind:this={items[i]}
      draggable="false"
      onpointerdown={(e) => onPointerDown(e, i)}
      onclick={(e) => onClick(e, i)}
      aria-label="Page {i + 1}"
      aria-pressed={selected.includes(i)}
    >
      <canvas
        bind:this={canvases[i]}
        style:width="{THUMB_W}px"
        style:height="{(THUMB_W * page.height) / page.width}px"
      ></canvas>
      <span>{i + 1}</span>
    </button>
  {/each}
</div>

<style>
  .thumbs {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 10px 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    outline: none;
  }
  .thumb {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 6px;
    border: 2px solid transparent;
    border-radius: 6px;
    background: none;
    min-height: 0;
  }
  .thumb:hover {
    background: var(--btn-hover);
  }
  .thumb.current {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .thumb.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .thumb.dragged {
    opacity: 0.45;
  }
  .reordering,
  .reordering .thumb {
    cursor: grabbing;
  }
  .thumb.drop-before::before,
  .thumb.drop-after::after {
    content: "";
    position: absolute;
    left: 4px;
    right: 4px;
    height: 3px;
    border-radius: 2px;
    background: var(--accent);
  }
  .thumb.drop-before::before {
    top: -5px;
  }
  .thumb.drop-after::after {
    bottom: -5px;
  }
  canvas {
    display: block;
    background: #fff;
    box-shadow: 0 1px 3px rgb(0 0 0 / 0.3);
  }
  span {
    font-size: 12px;
    color: var(--muted);
  }
</style>
