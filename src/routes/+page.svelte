<script lang="ts">
  import { onMount, tick } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import PdfView from "$lib/PdfView.svelte";
  import Thumbnails from "$lib/Thumbnails.svelte";
  import Outline from "$lib/Outline.svelte";
  import Icon from "$lib/Icon.svelte";
  import { printDocument } from "$lib/print";
  import {
    closeDocument,
    fileName,
    openDocument,
    outline as loadOutline,
    PASSWORD_REQUIRED,
    recentFiles,
    rememberRecent,
    search as runSearchApi,
    takeStartupFiles,
    type DocInfo,
    type OutlineItem,
    type SearchHit,
  } from "$lib/api";

  const APP_NAME = "Yet Another PDF Editor";
  const ZOOM_STEPS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8];

  interface Tab {
    key: number;
    doc: DocInfo;
    zoom: number;
    currentPage: number;
    outline: OutlineItem[] | null;
    query: string;
    searchedQuery: string;
    matchCase: boolean;
    hits: SearchHit[];
    activeHit: number;
  }

  let tabs: Tab[] = $state([]);
  let activeKey: number | null = $state(null);
  let nextKey = 1;
  const views: Record<number, PdfView> = $state({});
  const tab = $derived(tabs.find((t) => t.key === activeKey) ?? null);
  const view = $derived(tab ? views[tab.key] : undefined);

  let error = $state("");
  let dragging = $state(false);
  let recent = $state(recentFiles());

  let sidebarOpen = $state(readPref("sidebar", "1") === "1");
  let sidebarTab: "thumbs" | "outline" = $state("thumbs");

  let searchOpen = $state(false);
  let searching = $state(false);
  let searchInput: HTMLInputElement | undefined = $state();

  let printRoot: HTMLDivElement;
  let printing: { done: number; total: number; abort: AbortController } | null = $state(null);

  // Password prompt
  let passwordFor: string | null = $state(null);
  let password = $state("");
  let passwordWrong = $state(false);

  function readPref(key: string, fallback: string) {
    try {
      return localStorage.getItem(key) ?? fallback;
    } catch {
      return fallback;
    }
  }

  function writePref(key: string, value: string) {
    try {
      localStorage.setItem(key, value);
    } catch {
      /* non-persistent preference */
    }
  }

  async function load(path: string, pw?: string) {
    error = "";
    const existing = tabs.find((t) => t.doc.path.toLowerCase() === path.toLowerCase());
    if (existing) {
      activeKey = existing.key;
      return;
    }
    try {
      const doc = await openDocument(path, pw);
      const t: Tab = {
        key: nextKey++,
        doc,
        zoom: 1,
        currentPage: 0,
        outline: null,
        query: "",
        searchedQuery: "",
        matchCase: false,
        hits: [],
        activeHit: -1,
      };
      tabs.push(t);
      activeKey = t.key;
      passwordFor = null;
      recent = rememberRecent(path);
      loadOutline(doc.id).then(
        (o) => {
          const live = tabs.find((x) => x.key === t.key);
          if (live) live.outline = o;
        },
        () => {},
      );
    } catch (e) {
      if (e === PASSWORD_REQUIRED) {
        passwordWrong = pw !== undefined;
        passwordFor = path;
        password = "";
      } else {
        error = `Could not open ${fileName(path)}: ${e}`;
        if (String(e).includes("FileNotFound") || String(e).includes("os error 2")) {
          recent = rememberRecent(path, true);
        }
      }
    }
  }

  function closeTab(key: number) {
    const i = tabs.findIndex((t) => t.key === key);
    if (i < 0) return;
    const [t] = tabs.splice(i, 1);
    closeDocument(t.doc.id);
    delete views[key];
    if (activeKey === key) activeKey = (tabs[i] ?? tabs[i - 1])?.key ?? null;
  }

  function cycleTab(dir: 1 | -1) {
    if (tabs.length < 2) return;
    const i = tabs.findIndex((t) => t.key === activeKey);
    activeKey = tabs[(i + dir + tabs.length) % tabs.length].key;
  }

  async function pickFile() {
    const picked = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    for (const p of picked ?? []) await load(p);
  }

  function stepZoom(dir: 1 | -1) {
    if (!tab) return;
    const z = tab.zoom;
    const next =
      dir > 0
        ? (ZOOM_STEPS.find((s) => s > z + 0.001) ?? z)
        : ([...ZOOM_STEPS].reverse().find((s) => s < z - 0.001) ?? z);
    view?.setZoom(next);
  }

  function onZoomSelect(e: Event) {
    const sel = e.currentTarget as HTMLSelectElement;
    const v = sel.value;
    if (v === "width") view?.fitWidth();
    else if (v === "page") view?.fitPage();
    else view?.setZoom(Number(v));
    sel.value = "";
  }

  function onPageInput(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const n = Number(input.value);
    if (tab && Number.isFinite(n)) view?.goToPage(n - 1);
    input.value = String((tab?.currentPage ?? 0) + 1);
  }

  function toggleSidebar() {
    sidebarOpen = !sidebarOpen;
    writePref("sidebar", sidebarOpen ? "1" : "0");
  }

  // ---- Search ----

  async function openSearch() {
    if (!tab) return;
    searchOpen = true;
    await tick();
    searchInput?.select();
  }

  function closeSearch() {
    searchOpen = false;
    if (tab) {
      tab.hits = [];
      tab.activeHit = -1;
      tab.searchedQuery = "";
    }
  }

  async function findNext(dir: 1 | -1 = 1) {
    const t = tab;
    if (!t || !t.query) return;
    if (t.query !== t.searchedQuery || t.hits.length === 0) {
      searching = true;
      try {
        t.hits = await runSearchApi(t.doc.id, t.query, t.matchCase);
        t.searchedQuery = t.query;
      } finally {
        searching = false;
      }
      const after = t.hits.findIndex((h) => h.page >= t.currentPage);
      t.activeHit = t.hits.length ? (after < 0 ? 0 : after) : -1;
    } else {
      t.activeHit = (t.activeHit + dir + t.hits.length) % t.hits.length;
    }
    if (t.activeHit >= 0) views[t.key]?.scrollToHit(t.hits[t.activeHit]);
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      findNext(e.shiftKey ? -1 : 1);
    } else if (e.key === "Escape") {
      closeSearch();
    }
  }

  // ---- Print ----

  async function print() {
    if (!tab || printing) return;
    const abort = new AbortController();
    printing = { done: 0, total: tab.doc.pages.length, abort };
    try {
      await printDocument(tab.doc, printRoot, (done, total) => {
        if (printing) printing = { ...printing, done, total };
      }, abort.signal);
    } catch (e) {
      error = `Printing failed: ${e}`;
    } finally {
      printing = null;
    }
  }

  // ---- Keyboard ----

  function onKey(e: KeyboardEvent) {
    const typing = (e.target as HTMLElement)?.closest?.("input, select, textarea");
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === "F3") {
      e.preventDefault();
      if (tab?.query) findNext(e.shiftKey ? -1 : 1);
      else openSearch();
      return;
    }
    if (e.key === "Escape" && !typing) {
      view?.clearSelection();
      return;
    }
    if (!mod) return;
    const k = e.key.toLowerCase();
    let handled = true;
    if (k === "o") pickFile();
    else if (k === "w" && tab) closeTab(tab.key);
    else if (e.key === "Tab") cycleTab(e.shiftKey ? -1 : 1);
    else if (!tab) handled = false;
    else if (k === "f") openSearch();
    else if (k === "p") print();
    else if (k === "=" || k === "+") stepZoom(1);
    else if (k === "-") stepZoom(-1);
    else if (k === "0") view?.setZoom(1);
    else if (k === "b") toggleSidebar();
    else if (k === "c" && !typing) view?.copySelection();
    else if (k === "a" && !typing) view?.selectAll();
    else handled = false;
    if (handled) e.preventDefault();
  }

  $effect(() => {
    getCurrentWindow()
      .setTitle(tab ? `${fileName(tab.doc.path)} — ${APP_NAME}` : APP_NAME)
      .catch(() => {});
  });

  onMount(() => {
    // Block the webview's own context menu outside text fields (it offers "Reload" etc.).
    const noMenu = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest("input, textarea")) e.preventDefault();
    };
    window.addEventListener("contextmenu", noMenu);

    takeStartupFiles().then(async (files) => {
      for (const f of files) await load(f);
    });
    const unlistenFiles = listen<string[]>("open-files", async (e) => {
      for (const f of e.payload) await load(f);
    });
    const unlistenDrop = getCurrentWebview().onDragDropEvent(async (event) => {
      const p = event.payload;
      if (p.type === "enter" || p.type === "over") dragging = true;
      else if (p.type === "leave") dragging = false;
      else if (p.type === "drop") {
        dragging = false;
        for (const f of p.paths.filter((f) => f.toLowerCase().endsWith(".pdf"))) await load(f);
      }
    });
    return () => {
      window.removeEventListener("contextmenu", noMenu);
      unlistenFiles.then((f) => f());
      unlistenDrop.then((f) => f());
    };
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="app">
  {#if tabs.length}
    <nav class="tabs" aria-label="Open documents">
      {#each tabs as t (t.key)}
        <div class="tab" class:active={t.key === activeKey} title={t.doc.title ? `${t.doc.title}
${t.doc.path}` : t.doc.path}>
          <button class="tab-title" onclick={() => (activeKey = t.key)}>
            {fileName(t.doc.path)}
          </button>
          <button class="icon-btn small" onclick={() => closeTab(t.key)} title="Close (Ctrl+W)" aria-label="Close tab">
            <Icon name="close" size={14} />
          </button>
        </div>
      {/each}
      <button class="icon-btn small new-tab" onclick={pickFile} title="Open (Ctrl+O)" aria-label="Open file">
        <Icon name="plus" size={16} />
      </button>
    </nav>
  {/if}

  <header class="toolbar">
    {#if tab}
      <button class="icon-btn" class:on={sidebarOpen} onclick={toggleSidebar} title="Sidebar (Ctrl+B)" aria-label="Toggle sidebar">
        <Icon name="sidebar" />
      </button>
    {/if}
    <button class="icon-btn" onclick={pickFile} title="Open (Ctrl+O)" aria-label="Open file"><Icon name="open" /></button>
    {#if tab}
      <button class="icon-btn" onclick={print} disabled={!!printing} title="Print (Ctrl+P)" aria-label="Print">
        <Icon name="print" />
      </button>
      <span class="sep"></span>
      <button class="icon-btn" onclick={() => view?.goToPage(tab.currentPage - 1)} disabled={tab.currentPage === 0} title="Previous page" aria-label="Previous page">
        <Icon name="prev" />
      </button>
      <input
        class="page-input"
        type="text"
        inputmode="numeric"
        value={tab.currentPage + 1}
        onchange={onPageInput}
        aria-label="Page number"
      />
      <span class="muted">/ {tab.doc.pages.length}</span>
      <button class="icon-btn" onclick={() => view?.goToPage(tab.currentPage + 1)} disabled={tab.currentPage >= tab.doc.pages.length - 1} title="Next page" aria-label="Next page">
        <Icon name="next" />
      </button>
      <span class="sep"></span>
      <button class="icon-btn" onclick={() => stepZoom(-1)} title="Zoom out (Ctrl+-)" aria-label="Zoom out"><Icon name="minus" /></button>
      <select onchange={onZoomSelect} aria-label="Zoom">
        <option value="" selected hidden>{Math.round(tab.zoom * 100)}%</option>
        <option value="width">Fit width</option>
        <option value="page">Fit page</option>
        {#each ZOOM_STEPS as z}<option value={z}>{z * 100}%</option>{/each}
      </select>
      <button class="icon-btn" onclick={() => stepZoom(1)} title="Zoom in (Ctrl++)" aria-label="Zoom in"><Icon name="plus" /></button>

      <span class="spacer"></span>
      {#if searchOpen}
        <div class="search" role="search">
          <input
            bind:this={searchInput}
            bind:value={tab.query}
            onkeydown={onSearchKey}
            placeholder="Find in document"
            aria-label="Find in document"
          />
          <span class="count muted">
            {#if searching}Searching…
            {:else if tab.searchedQuery && tab.searchedQuery === tab.query}
              {tab.hits.length ? `${tab.activeHit + 1} of ${tab.hits.length}` : "No results"}
            {/if}
          </span>
          <label class="case" title="Match case">
            <input
              type="checkbox"
              bind:checked={tab.matchCase}
              onchange={() => (tab.searchedQuery = "")}
            />Aa
          </label>
          <button class="icon-btn" onclick={() => findNext(-1)} title="Previous (Shift+Enter)" aria-label="Previous match"><Icon name="up" /></button>
          <button class="icon-btn" onclick={() => findNext(1)} title="Next (Enter)" aria-label="Next match"><Icon name="down" /></button>
          <button class="icon-btn" onclick={closeSearch} title="Close (Esc)" aria-label="Close search"><Icon name="close" /></button>
        </div>
      {:else}
        <button class="icon-btn" onclick={openSearch} title="Find (Ctrl+F)" aria-label="Find"><Icon name="search" /></button>
      {/if}
    {/if}
  </header>

  {#if error}
    <div class="error" role="alert">
      {error}
      <button class="link" onclick={() => (error = "")}>Dismiss</button>
    </div>
  {/if}

  <div class="body">
    {#if tab && sidebarOpen}
      <aside class="sidebar">
        <div class="side-tabs" role="tablist">
          <button role="tab" aria-selected={sidebarTab === "thumbs"} class:on={sidebarTab === "thumbs"} onclick={() => (sidebarTab = "thumbs")}>Pages</button>
          <button role="tab" aria-selected={sidebarTab === "outline"} class:on={sidebarTab === "outline"} onclick={() => (sidebarTab = "outline")}>Bookmarks</button>
        </div>
        <div class="side-body">
          {#key tab.key}
            {#if sidebarTab === "thumbs"}
              <Thumbnails doc={tab.doc} currentPage={tab.currentPage} onselect={(p) => view?.goToPage(p)} />
            {:else if tab.outline?.length}
              <div class="outline-scroll">
                <Outline items={tab.outline} onselect={(p) => view?.goToPage(p)} />
              </div>
            {:else}
              <p class="muted side-empty">{tab.outline ? "This document has no bookmarks." : "Loading…"}</p>
            {/if}
          {/key}
        </div>
      </aside>
    {/if}

    <main class="viewer">
      {#each tabs as t (t.key)}
        <div class="view-slot" class:hidden={t.key !== activeKey}>
          <PdfView
            doc={t.doc}
            bind:zoom={t.zoom}
            bind:currentPage={t.currentPage}
            searchHits={t.hits}
            activeHit={t.activeHit}
            bind:this={views[t.key]}
          />
        </div>
      {/each}
      {#if !tab}
        <div class="empty">
          <img src="/app-icon.png" alt="" width="96" height="96" />
          <h1>{APP_NAME}</h1>
          <button class="primary" onclick={pickFile}>Open file…</button>
          <p class="muted">or drop PDFs anywhere in this window</p>
          {#if recent.length}
            <section class="recent" aria-label="Recent files">
              <h2>Recent</h2>
              {#each recent as path}
                <div class="recent-row">
                  <button class="recent-item" onclick={() => load(path)} title={path}>
                    <Icon name="file" size={16} />
                    <span class="recent-name">{fileName(path)}</span>
                    <span class="recent-path muted">{path}</span>
                  </button>
                  <button class="icon-btn small" title="Remove from list" aria-label="Remove from recent files" onclick={() => (recent = rememberRecent(path, true))}>
                    <Icon name="close" size={14} />
                  </button>
                </div>
              {/each}
            </section>
          {/if}
        </div>
      {/if}
      {#if dragging}<div class="drop">Drop PDF to open</div>{/if}
    </main>
  </div>
</div>

<div class="print-root" bind:this={printRoot}></div>

{#if printing}
  <div class="backdrop">
    <div class="dialog">
      <h2>Preparing to print…</h2>
      <progress max={printing.total} value={printing.done}></progress>
      <p class="muted">Page {Math.min(printing.done + 1, printing.total)} of {printing.total}</p>
      <div class="actions">
        <button onclick={() => printing?.abort.abort()}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if passwordFor}
  <div class="backdrop">
    <form
      class="dialog"
      onsubmit={(e) => {
        e.preventDefault();
        load(passwordFor!, password);
      }}
    >
      <h2>Password required</h2>
      <p class="muted">{fileName(passwordFor)} is protected.</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input type="password" bind:value={password} autofocus aria-label="Password" />
      {#if passwordWrong}<p class="bad">Incorrect password, try again.</p>{/if}
      <div class="actions">
        <button type="button" onclick={() => (passwordFor = null)}>Cancel</button>
        <button type="submit" class="primary">Open</button>
      </div>
    </form>
  </div>
{/if}

<style>
  :global(:root) {
    --bg: #f6f6f7;
    --bg-2: #ebebef;
    --fg: #1d1d1f;
    --muted: #6b6b70;
    --border: #d9d9de;
    --btn: #ffffff;
    --btn-hover: #e4e4ea;
    --accent: #6a4cff;
    --viewer-bg: #e3e3e8;
    --danger: #c62828;
    color-scheme: light dark;
    font-family: system-ui, "Segoe UI", sans-serif;
    font-size: 14px;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --bg: #232327;
      --bg-2: #1b1b1f;
      --fg: #ececf0;
      --muted: #9a9aa2;
      --border: #3a3a40;
      --btn: #2d2d33;
      --btn-hover: #3a3a42;
      --accent: #8d75ff;
      --viewer-bg: #161619;
      --danger: #ef6b6b;
    }
  }
  :global(html, body) {
    margin: 0;
    height: 100%;
    background: var(--bg);
    color: var(--fg);
    overflow: hidden;
  }
  :global(button, select, input) {
    font: inherit;
    color: inherit;
  }
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  /* Tabs */
  .tabs {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    padding: 6px 8px 0;
    background: var(--bg-2);
    overflow-x: auto;
    flex: none;
  }
  .tab {
    display: flex;
    align-items: center;
    max-width: 220px;
    min-width: 0;
    padding: 0 4px 0 2px;
    border-radius: 8px 8px 0 0;
    border: 1px solid transparent;
    border-bottom: none;
  }
  .tab.active {
    background: var(--bg);
    border-color: var(--border);
  }
  .tab-title {
    flex: 1;
    min-width: 0;
    border: none;
    background: none;
    padding: 7px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }
  .tab:not(.active) .tab-title {
    color: var(--muted);
  }
  .new-tab {
    margin: 0 0 4px 4px;
  }

  /* Toolbar */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    flex: none;
    min-height: 34px;
  }
  .sep {
    width: 1px;
    height: 20px;
    background: var(--border);
    margin: 0 6px;
  }
  .spacer {
    flex: 1;
  }
  button,
  select,
  input {
    background: var(--btn);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 10px;
    min-height: 30px;
    box-sizing: border-box;
  }
  button:hover:not(:disabled) {
    background: var(--btn-hover);
  }
  button:disabled {
    opacity: 0.4;
  }
  .icon-btn {
    display: inline-grid;
    place-items: center;
    width: 30px;
    padding: 0;
    border-color: transparent;
    background: transparent;
  }
  .icon-btn.small {
    width: 24px;
    min-height: 24px;
  }
  .icon-btn.on {
    background: var(--btn-hover);
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .primary:hover:not(:disabled) {
    background: var(--accent);
    filter: brightness(1.1);
  }
  .link {
    border: none;
    background: none;
    text-decoration: underline;
  }
  .page-input {
    width: 3.5em;
    text-align: center;
    padding: 4px;
  }
  .muted {
    color: var(--muted);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .search > input {
    width: 200px;
  }
  .count {
    min-width: 70px;
    font-size: 12px;
    text-align: right;
  }
  .case {
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: 12px;
    color: var(--muted);
  }
  .case input {
    min-height: 0;
  }

  /* Body */
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .sidebar {
    width: 190px;
    flex: none;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    background: var(--bg);
  }
  .side-tabs {
    display: flex;
    gap: 4px;
    padding: 6px;
    border-bottom: 1px solid var(--border);
  }
  .side-tabs button {
    flex: 1;
    border-color: transparent;
    background: transparent;
    font-size: 13px;
  }
  .side-tabs button.on {
    background: var(--btn-hover);
  }
  .side-body {
    flex: 1;
    min-height: 0;
  }
  .outline-scroll {
    height: 100%;
    overflow: auto;
  }
  .side-empty {
    padding: 12px;
    margin: 0;
  }
  .viewer {
    position: relative;
    flex: 1;
    min-width: 0;
    background: var(--viewer-bg);
  }
  .view-slot {
    position: absolute;
    inset: 0;
  }
  .view-slot.hidden {
    display: none;
  }

  /* Empty state */
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 16px;
    box-sizing: border-box;
    overflow: auto;
  }
  .empty h1 {
    font-size: 22px;
    font-weight: 600;
    margin: 0 0 6px;
  }
  .empty p {
    margin: 0;
  }
  .recent {
    margin-top: 18px;
    width: min(520px, 100%);
  }
  .recent h2 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    margin: 0 0 6px 8px;
  }
  .recent-row {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .recent-item {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    border-color: transparent;
    background: transparent;
    text-align: left;
  }
  .recent-name {
    white-space: nowrap;
  }
  .recent-path {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .drop {
    position: absolute;
    inset: 8px;
    border: 3px dashed var(--accent);
    border-radius: 12px;
    display: grid;
    place-items: center;
    font-size: 18px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    pointer-events: none;
  }
  .error {
    padding: 8px 12px;
    background: color-mix(in srgb, var(--danger) 15%, var(--bg));
    color: var(--danger);
    border-bottom: 1px solid var(--border);
  }

  /* Dialogs */
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: center;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 18px 20px;
    width: min(360px, calc(100vw - 32px));
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dialog h2 {
    margin: 0;
    font-size: 16px;
  }
  .dialog p {
    margin: 0;
  }
  progress {
    width: 100%;
  }
  .bad {
    color: var(--danger);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  /* Printing: only the rasterised pages are visible. */
  .print-root {
    display: none;
  }
  @media print {
    @page {
      margin: 0;
    }
    .app,
    .backdrop {
      display: none !important;
    }
    :global(html, body) {
      height: auto;
      overflow: visible;
      background: #fff;
    }
    .print-root {
      display: block;
    }
    .print-root :global(img) {
      display: block;
      width: 100%;
      height: 100vh;
      object-fit: contain;
      break-after: page;
    }
  }
</style>
