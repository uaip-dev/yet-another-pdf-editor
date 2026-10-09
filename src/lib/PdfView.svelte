<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    pageLinks,
    pageText,
    renderPage,
    textOf,
    type DocInfo,
    type Link,
    type PageText,
    type SearchHit,
  } from "./api";
  import {
    hitTest,
    lineAt,
    ordered,
    rangeRects,
    selectionOnPage,
    wordAt,
    type Caret,
    type Selection,
  } from "./textgeom";

  interface Props {
    doc: DocInfo;
    /** 1 = 100% (one PDF point = 96/72 CSS px). */
    zoom: number;
    currentPage: number;
    searchHits?: SearchHit[];
    activeHit?: number;
  }

  let {
    doc,
    zoom = $bindable(1),
    currentPage = $bindable(0),
    searchHits = [],
    activeHit = -1,
  }: Props = $props();

  const PT_TO_PX = 96 / 72;
  const GAP = 12;
  const MIN_ZOOM = 0.1;
  const MAX_ZOOM = 8;

  let scroller: HTMLDivElement;
  let pageEls: HTMLDivElement[] = $state([]);
  let canvases: HTMLCanvasElement[] = $state([]);

  // Pages within the observer margin get canvases, text and links.
  const nearPages = new SvelteSet<number>();
  const texts = new SvelteMap<number, PageText>();
  const links = new SvelteMap<number, Link[]>();

  let selection: Selection | null = $state(null);
  let dragging = false;

  // Non-reactive render bookkeeping.
  const renderedWidth = new Map<number, number>(); // page -> bitmap px width on canvas
  const inflight = new Map<number, number>(); // page -> px width being rendered
  let rerenderTimer: ReturnType<typeof setTimeout> | undefined;

  const cssWidth = (i: number) => doc.pages[i].width * zoom * PT_TO_PX;
  const cssHeight = (i: number) => doc.pages[i].height * zoom * PT_TO_PX;

  /** Search hits grouped by page, keeping their global index for "active". */
  const hitsByPage = $derived.by(() => {
    const m = new Map<number, { hit: SearchHit; n: number }[]>();
    searchHits.forEach((hit, n) => {
      let list = m.get(hit.page);
      if (!list) m.set(hit.page, (list = []));
      list.push({ hit, n });
    });
    return m;
  });

  async function ensureRendered(i: number) {
    const target = Math.round(cssWidth(i) * devicePixelRatio);
    if (renderedWidth.get(i) === target || inflight.get(i) === target) return;
    inflight.set(i, target);
    try {
      const bmp = await renderPage(doc.id, i, target);
      // Drop the result if the page scrolled away or zoom changed meanwhile.
      if (inflight.get(i) !== target || !nearPages.has(i)) return bmp.close();
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

  function loadOverlay(i: number) {
    if (!texts.has(i)) pageText(doc.id, i).then((t) => texts.set(i, t), console.error);
    if (!links.has(i)) pageLinks(doc.id, i).then((l) => links.set(i, l), console.error);
  }

  function release(i: number) {
    const canvas = canvases[i];
    if (canvas) canvas.width = canvas.height = 0; // frees the backing store
    renderedWidth.delete(i);
    inflight.delete(i);
  }

  /** Visible pages first, so the page in view appears before its neighbours. */
  function renderNear() {
    [...nearPages]
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

  export async function scrollToHit(hit: SearchHit) {
    const t = await pageText(doc.id, hit.page);
    const r = rangeRects(t, hit.start, hit.start + hit.len)[0];
    if (!r) return goToPage(hit.page);
    const scale = zoom * PT_TO_PX;
    const el = pageEls[hit.page];
    const top = el.offsetTop + r[1] * scale - scroller.clientHeight / 3;
    const x0 = el.offsetLeft + r[0] * scale;
    const x1 = el.offsetLeft + r[2] * scale;
    const visibleX = x0 >= scroller.scrollLeft && x1 <= scroller.scrollLeft + scroller.clientWidth;
    scroller.scrollTo({
      top,
      left: visibleX ? scroller.scrollLeft : x0 - scroller.clientWidth / 3,
    });
  }

  // ---- Text selection ----

  export function hasSelection() {
    return selection !== null;
  }

  export function clearSelection() {
    selection = null;
  }

  export async function selectAll() {
    const last = doc.pages.length - 1;
    const t = await pageText(doc.id, last);
    selection = { anchor: { page: 0, index: 0 }, focus: { page: last, index: t.codes.length } };
  }

  /** Copies the selected text; returns false if nothing is selected. */
  export async function copySelection(): Promise<boolean> {
    if (!selection) return false;
    const sel = selection;
    const [a, b] = ordered(sel);
    const parts: string[] = [];
    for (let p = a.page; p <= b.page; p++) {
      const t = await pageText(doc.id, p);
      const range = selectionOnPage(sel, p, t.codes.length);
      if (range) parts.push(textOf(t, range[0], range[1]));
    }
    const text = parts.join("\n").replace(/\r\n?/g, "\n");
    if (!text) return false;
    await navigator.clipboard.writeText(text);
    return true;
  }

  function caretAt(e: PointerEvent | MouseEvent, i: number): Caret | null {
    const t = texts.get(i);
    if (!t) return null;
    const r = pageEls[i].getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * doc.pages[i].width;
    const y = ((e.clientY - r.top) / r.height) * doc.pages[i].height;
    return { page: i, index: hitTest(t, x, y) };
  }

  function onPagePointerDown(e: PointerEvent, i: number) {
    if (e.button !== 0) return;
    e.preventDefault(); // no native drag / text selection
    scroller.focus({ preventScroll: true });
    const caret = caretAt(e, i);
    if (!caret) {
      selection = null;
      return;
    }
    const t = texts.get(i)!;
    if (e.detail === 2 || e.detail === 3) {
      const [s, end] = e.detail === 2 ? wordAt(t, caret.index) : lineAt(t, caret.index);
      selection = { anchor: { page: i, index: s }, focus: { page: i, index: end } };
      return;
    }
    selection =
      e.shiftKey && selection
        ? { anchor: selection.anchor, focus: caret }
        : { anchor: caret, focus: caret };
    dragging = true;
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging || !selection) return;
    // Auto-scroll while dragging near the top or bottom edge.
    const r = scroller.getBoundingClientRect();
    if (e.clientY < r.top + 24) scroller.scrollBy(0, -24);
    else if (e.clientY > r.bottom - 24) scroller.scrollBy(0, 24);
    const pageEl = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest<HTMLElement>(".page");
    if (!pageEl || !scroller.contains(pageEl)) return;
    const caret = caretAt(e, Number(pageEl.dataset.index));
    if (caret) selection = { anchor: selection.anchor, focus: caret };
  }

  function onPointerUp() {
    if (!dragging) return;
    dragging = false;
    if (selection && ordered(selection)[0].page === ordered(selection)[1].page &&
        selection.anchor.index === selection.focus.index) {
      selection = null;
    }
  }

  function followLink(e: MouseEvent, link: Link) {
    e.stopPropagation();
    if (link.page !== null) goToPage(link.page);
    else if (link.uri) openUrl(link.uri).catch(console.error);
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
            nearPages.add(i);
            ensureRendered(i);
            loadOverlay(i);
          } else {
            nearPages.delete(i);
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

<svelte:window onpointermove={onPointerMove} onpointerup={onPointerUp} />

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="scroller" bind:this={scroller} onscroll={onScroll} onwheel={onWheel} tabindex="0">
  {#each doc.pages as page, i (i)}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="page"
      data-index={i}
      bind:this={pageEls[i]}
      style:width="{cssWidth(i)}px"
      style:height="{cssHeight(i)}px"
      onpointerdown={(e) => onPagePointerDown(e, i)}
    >
      <canvas bind:this={canvases[i]}></canvas>
      {#if nearPages.has(i)}
        {@const t = texts.get(i)}
        <svg class="overlay" viewBox="0 0 {page.width} {page.height}" preserveAspectRatio="none">
          {#if t}
            {#each hitsByPage.get(i) ?? [] as { hit, n } (n)}
              {#each rangeRects(t, hit.start, hit.start + hit.len) as r}
                <rect class="hit" class:active={n === activeHit} x={r[0]} y={r[1]} width={r[2] - r[0]} height={r[3] - r[1]} />
              {/each}
            {/each}
            {#if selection}
              {@const range = selectionOnPage(selection, i, t.codes.length)}
              {#if range}
                {#each rangeRects(t, range[0], range[1]) as r}
                  <rect class="sel" x={r[0]} y={r[1]} width={r[2] - r[0]} height={r[3] - r[1]} />
                {/each}
              {/if}
            {/if}
          {/if}
          {#each links.get(i) ?? [] as link}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <rect
              class="link"
              role="link"
              tabindex="-1"
              x={link.rect[0]}
              y={link.rect[1]}
              width={link.rect[2] - link.rect[0]}
              height={link.rect[3] - link.rect[1]}
              onpointerdown={(e) => e.stopPropagation()}
              onclick={(e) => followLink(e, link)}
            >
              <title>{link.uri ?? `Go to page ${(link.page ?? 0) + 1}`}</title>
            </rect>
          {/each}
        </svg>
      {/if}
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
    outline: none;
  }
  .page {
    flex: none;
    position: relative;
    background: #fff;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.25);
    cursor: text;
    user-select: none;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }
  .overlay {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  .sel {
    fill: rgb(47 111 222 / 0.32);
  }
  .hit {
    fill: rgb(255 200 0 / 0.4);
  }
  .hit.active {
    fill: rgb(255 120 0 / 0.55);
  }
  .link {
    fill: transparent;
    cursor: pointer;
  }
  .link:hover {
    fill: rgb(47 111 222 / 0.1);
  }
</style>
