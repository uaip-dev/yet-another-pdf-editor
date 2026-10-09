<script lang="ts">
  import { onMount } from "svelte";
  import { renderPage, type DocInfo } from "./api";

  interface Props {
    doc: DocInfo;
    currentPage: number;
    onselect: (page: number) => void;
  }

  let { doc, currentPage, onselect }: Props = $props();

  const THUMB_W = 120;

  let scroller: HTMLDivElement;
  let items: HTMLButtonElement[] = $state([]);
  let canvases: HTMLCanvasElement[] = $state([]);
  const drawn = new Set<number>();

  async function draw(i: number) {
    if (drawn.has(i)) return;
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

  // Keep the current page's thumbnail in view.
  $effect(() => {
    items[currentPage]?.scrollIntoView({ block: "nearest" });
  });

  onMount(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const i = Number((entry.target as HTMLElement).dataset.index);
          if (entry.isIntersecting) draw(i);
          else if (drawn.delete(i) && canvases[i]) canvases[i].width = canvases[i].height = 0;
        }
      },
      { root: scroller, rootMargin: "300px 0px" },
    );
    items.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  });
</script>

<div class="thumbs" bind:this={scroller}>
  {#each doc.pages as page, i (i)}
    <button
      class="thumb"
      class:current={i === currentPage}
      data-index={i}
      bind:this={items[i]}
      onclick={() => onselect(i)}
      aria-label="Page {i + 1}"
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
    height: 100%;
    overflow-y: auto;
    padding: 10px 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }
  .thumb {
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
    border-color: var(--accent);
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
