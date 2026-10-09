<script lang="ts">
  import Outline from "./Outline.svelte";
  import type { OutlineItem } from "./api";

  interface Props {
    items: OutlineItem[];
    onselect: (page: number) => void;
    depth?: number;
  }

  let { items, onselect, depth = 0 }: Props = $props();

  // Top level starts expanded; nested levels start collapsed.
  let open: Record<number, boolean> = $state({});
</script>

<ul class:root={depth === 0}>
  {#each items as item, i}
    {@const expanded = open[i] ?? depth === 0}
    <li>
      <div class="row" style:padding-left="{depth * 14 + 4}px">
        {#if item.children.length}
          <button
            class="twisty"
            onclick={() => (open[i] = !expanded)}
            aria-label={expanded ? "Collapse" : "Expand"}
            aria-expanded={expanded}>{expanded ? "▾" : "▸"}</button
          >
        {:else}
          <span class="twisty"></span>
        {/if}
        <button
          class="title"
          disabled={item.page === null}
          title={item.title}
          onclick={() => item.page !== null && onselect(item.page)}
        >
          {item.title || "(untitled)"}
        </button>
      </div>
      {#if item.children.length && expanded}
        <Outline items={item.children} {onselect} depth={depth + 1} />
      {/if}
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  ul.root {
    padding: 8px 4px;
  }
  .row {
    display: flex;
    align-items: center;
  }
  button {
    border: none;
    background: none;
    min-height: 26px;
    padding: 2px 6px;
    text-align: left;
  }
  .twisty {
    width: 22px;
    flex: none;
    padding: 0;
    color: var(--muted);
    text-align: center;
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: 4px;
  }
  .title:hover:not(:disabled) {
    background: var(--btn-hover);
  }
</style>
