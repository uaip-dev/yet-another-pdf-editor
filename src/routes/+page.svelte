<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import PdfView from "$lib/PdfView.svelte";
  import {
    closeDocument,
    fileName,
    openDocument,
    PASSWORD_REQUIRED,
    type DocInfo,
  } from "$lib/api";

  let doc: DocInfo | null = $state(null);
  let zoom = $state(1);
  let currentPage = $state(0);
  let view: PdfView | undefined = $state();
  let error = $state("");
  let dragging = $state(false);

  // Password prompt
  let passwordFor: string | null = $state(null);
  let password = $state("");
  let passwordWrong = $state(false);

  const ZOOM_STEPS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8];

  async function load(path: string, pw?: string) {
    error = "";
    try {
      const next = await openDocument(path, pw);
      if (doc) closeDocument(doc.id);
      doc = next;
      currentPage = 0;
      zoom = 1;
      passwordFor = null;
      getCurrentWindow().setTitle(`${doc.title ?? fileName(path)} — PDF Editor`);
    } catch (e) {
      if (e === PASSWORD_REQUIRED) {
        passwordWrong = pw !== undefined;
        passwordFor = path;
        password = "";
      } else {
        error = `Could not open ${fileName(path)}: ${e}`;
      }
    }
  }

  async function pickFile() {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (path) load(path);
  }

  function stepZoom(dir: 1 | -1) {
    const next =
      dir > 0
        ? (ZOOM_STEPS.find((z) => z > zoom + 0.001) ?? zoom)
        : ([...ZOOM_STEPS].reverse().find((z) => z < zoom - 0.001) ?? zoom);
    view?.setZoom(next);
  }

  function onZoomSelect(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v === "width") view?.fitWidth();
    else if (v === "page") view?.fitPage();
    else view?.setZoom(Number(v));
  }

  function onPageInput(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const n = Number(input.value);
    if (Number.isFinite(n)) view?.goToPage(n - 1);
    input.value = String(currentPage + 1);
  }

  function onKey(e: KeyboardEvent) {
    if (!e.ctrlKey && !e.metaKey) return;
    if (e.key === "o") pickFile();
    else if (!doc) return;
    else if (e.key === "=" || e.key === "+") stepZoom(1);
    else if (e.key === "-") stepZoom(-1);
    else if (e.key === "0") view?.setZoom(1);
    else return;
    e.preventDefault();
  }

  onMount(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === "enter" || p.type === "over") dragging = true;
      else if (p.type === "leave") dragging = false;
      else if (p.type === "drop") {
        dragging = false;
        const pdf = p.paths.find((f) => f.toLowerCase().endsWith(".pdf"));
        if (pdf) load(pdf);
      }
    });
    return () => void unlisten.then((f) => f());
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="app">
  <header class="toolbar">
    <button onclick={pickFile} title="Open (Ctrl+O)">Open…</button>
    {#if doc}
      <span class="sep"></span>
      <button onclick={() => view?.goToPage(currentPage - 1)} disabled={currentPage === 0} title="Previous page">‹</button>
      <input
        class="page-input"
        type="text"
        inputmode="numeric"
        value={currentPage + 1}
        onchange={onPageInput}
        aria-label="Page number"
      />
      <span class="muted">/ {doc.pages.length}</span>
      <button onclick={() => view?.goToPage(currentPage + 1)} disabled={currentPage >= doc.pages.length - 1} title="Next page">›</button>
      <span class="sep"></span>
      <button onclick={() => stepZoom(-1)} title="Zoom out (Ctrl+-)">−</button>
      <select onchange={onZoomSelect} aria-label="Zoom">
        <option value="" selected hidden>{Math.round(zoom * 100)}%</option>
        <option value="width">Fit width</option>
        <option value="page">Fit page</option>
        {#each ZOOM_STEPS as z}<option value={z}>{z * 100}%</option>{/each}
      </select>
      <button onclick={() => stepZoom(1)} title="Zoom in (Ctrl++)">+</button>
      <span class="title muted">{doc.title ?? fileName(doc.path)}</span>
    {/if}
  </header>

  {#if error}
    <div class="error" role="alert">
      {error}
      <button class="link" onclick={() => (error = "")}>Dismiss</button>
    </div>
  {/if}

  <main class="viewer">
    {#if doc}
      {#key doc.id}
        <PdfView {doc} bind:zoom bind:currentPage bind:this={view} />
      {/key}
    {:else}
      <div class="empty">
        <p>Open a PDF to get started</p>
        <button class="primary" onclick={pickFile}>Open file…</button>
        <p class="muted">or drop a PDF anywhere in this window</p>
      </div>
    {/if}
    {#if dragging}<div class="drop">Drop PDF to open</div>{/if}
  </main>
</div>

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
    --fg: #1d1d1f;
    --muted: #6b6b70;
    --border: #d9d9de;
    --btn: #ffffff;
    --btn-hover: #ececf0;
    --accent: #2f6fde;
    --viewer-bg: #e3e3e8;
    --danger: #c62828;
    color-scheme: light dark;
    font-family: system-ui, "Segoe UI", sans-serif;
    font-size: 14px;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --bg: #222226;
      --fg: #ececf0;
      --muted: #9a9aa2;
      --border: #3a3a40;
      --btn: #2d2d33;
      --btn-hover: #3a3a42;
      --viewer-bg: #18181b;
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
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    flex: none;
  }
  .sep {
    width: 1px;
    height: 20px;
    background: var(--border);
    margin: 0 4px;
  }
  .title {
    margin-left: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40%;
  }
  button,
  select,
  input {
    font: inherit;
    color: inherit;
    background: var(--btn);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 10px;
    min-height: 28px;
  }
  button:hover:not(:disabled) {
    background: var(--btn-hover);
  }
  button:disabled {
    opacity: 0.4;
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
  .viewer {
    position: relative;
    flex: 1;
    min-height: 0;
    background: var(--viewer-bg);
  }
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
  }
  .empty p {
    margin: 0;
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
  .bad {
    color: var(--danger);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
