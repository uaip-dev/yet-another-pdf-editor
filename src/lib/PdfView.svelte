<script lang="ts">
  import { onMount } from "svelte";
  import { renderPage, type DocInfo } from "./api";

  interface Props {
    doc: DocInfo;
    /** 1 = 100% (one PDF point = 96/72 CSS px). */
    zoom: number;
    currentPage: number;
  }

  let { doc, zoom = $bindable(1), currentPage = $bindable(0) }: Props = $props();

  const PT_TO_PX = 96 / 72;
  const GAP = 12;
  const MIN_ZOOM = 0.1;
  const MAX_ZOOM = 8;

  let scroller: HTMLDivElement;
  let pageEls: HTMLDivElement[] = $state([]);
  let canvases: HTMLCanvasElement[] = $state([]);

  // Non-reactive render bookkeeping.
  const near = new Set<number>(); // pages within the observer's margin
  const renderedWidth = new Map<number, number>(); // page -> bitmap px width on canvas
  const inflight = new Map<number, number>(); // page -> px width being rendered
  let rerenderTimer: ReturnType<typeof setTimeout> | undefined;

  const cssWidth = (i: number) => doc.pages[i].width * zoom * PT_TO_PX;
  const cssHeight = (i: number) => doc.pages[i].height * zoom * PT_TO_PX;

  async function ensureRendered(i: number) {
    const target = Math.round(cssWidth(i) * devicePixelRatio);
    if (renderedWidth.get(i) === target || inflight.get(i) === target) return;
    inflight.set(i, target);
    try {
      const bmp = await renderPage(doc.id, i, target);
      // Drop the result if the page scrolled away or zoom changed meanwhile.
      if (inflight.get(i) !== target || !near.has(i)) return bmp.close();
      const canvas = canvases[i];
      canvas.width = bmp.width;
      canvas.height = bmp.height;
      canvas.getContext("2d")!.drawImage(bmp, 0, 0);
      bmp.close();
      renderedWidth.set(i, target);
    } catch (e) {
      console.error(`render page ${i + 1} failed`, e);
    } finally {
      if (inflight.get(i) === target) inflight.delete(i);
    }
  }

  function release(i: number) {
    const canvas = canvases[i];
    if (canvas) canvas.width = canvas.height = 0; // frees the backing store
    renderedWidth.delete(i);
    inflight.delete(i);
  }

  /** Visible pages first, so the page in view appears before its neighbours. */
  function renderNear() {
    [...near]
      .sort((a, b) => Math.abs(a - currentPage) - Math.abs(b - currentPage))
      .forEach(ensureRendered);
  }

  function updateCurrentPage() {
    const probe = scroller.scrollTop + scroller.clientHeight / 3;
    let lo = 0;
    let hi = pageEls.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (pageEls[mid].offsetTop <= probe) lo = mid;
      else hi = mid - 1;
    }
    currentPage = lo;
  }

  let scrollFrame = 0;
  function onScroll() {
    cancelAnimationFrame(scrollFrame);
    scrollFrame = requestAnimationFrame(updateCurrentPage);
  }

  /** Changes zoom while keeping the point under (clientX, clientY) fixed. */
  export function setZoom(next: number, clientX?: number, clientY?: number) {
    next = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, next));
    if (next === zoom) return;
    const rect = scroller.getBoundingClientRect();
    const ox = (clientX ?? rect.left + rect.width / 2) - rect.left;
    const oy = (clientY ?? rect.top + rect.height / 3) - rect.top;
    const ratio = next / zoom;
    const x = (scroller.scrollLeft + ox) * ratio - ox;
    const y = (scroller.scrollTop + oy - GAP) * ratio + GAP - oy;
    zoom = next;
    requestAnimationFrame(() => scroller.scrollTo(x, y));
  }

  export function fitWidth() {
    const widest = Math.max(...doc.pages.map((p) => p.width));
    setZoom((scroller.clientWidth - 2 * GAP - 16) / (widest * PT_TO_PX));
  }

  export function fitPage() {
    const p = doc.pages[currentPage];
    const zw = (scroller.clientWidth - 2 * GAP - 16) / (p.width * PT_TO_PX);
    const zh = (scroller.clientHeight - 2 * GAP) / (p.height * PT_TO_PX);
    setZoom(Math.min(zw, zh));
    goToPage(currentPage);
  }

  export function goToPage(i: number) {
    i = Math.min(doc.pages.length - 1, Math.max(0, i));
    requestAnimationFrame(() => scroller.scrollTo({ top: pageEls[i].offsetTop - GAP }));
  }

  function onWheel(e: WheelEvent) {
    if (!e.ctrlKey) return;
    e.preventDefault();
    setZoom(zoom * Math.exp(-e.deltaY * 0.0015), e.clientX, e.clientY);
  }

  // After a zoom change the canvases are CSS-stretched immediately;
  // re-render them sharply once zooming settles.
  $effect(() => {
    void zoom;
    clearTimeout(rerenderTimer);
    rerenderTimer = setTimeout(renderNear, 120);
  });

  onMount(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const i = Number((entry.target as HTMLElement).dataset.index);
          if (entry.isIntersecting) {
            near.add(i);
            ensureRendered(i);
          } else {
            near.delete(i);
            release(i);
          }
        }
      },
      { root: scroller, rootMargin: "100% 0px" },
    );
    pageEls.forEach((el) => observer.observe(el));
    return () => {
      observer.disconnect();
      clearTimeout(rerenderTimer);
    };
  });
</script>

<div class="scroller" bind:this={scroller} onscroll={onScroll} onwheel={onWheel}>
  {#each doc.pages as _, i (i)}
    <div
      class="page"
      data-index={i}
      bind:this={pageEls[i]}
      style:width="{cssWidth(i)}px"
      style:height="{cssHeight(i)}px"
    >
      <canvas bind:this={canvases[i]}></canvas>
    </div>
  {/each}
</div>

<style>
  .scroller {
    position: absolute;
    inset: 0;
    overflow: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
    align-items: safe center;
    gap: 12px;
    background: var(--viewer-bg);
  }
  .page {
    flex: none;
    position: relative;
    background: #fff;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.25);
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
