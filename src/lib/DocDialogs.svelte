<script lang="ts" module>
  export type DocDialog = "properties" | "protect" | "compress" | "export";
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    compressDocument,
    documentProperties,
    documentProtection,
    exportImages,
    formatBytes,
    pageLabel,
    setDocumentProperties,
    setDocumentProtection,
    type CompressReport,
    type DocState,
    type Properties,
    type Protection,
  } from "./api";

  interface Props {
    kind: DocDialog;
    docId: number;
    pageCount: number;
    /** Pages the user selected (or the current page). */
    pages: number[];
    onstate: (s: DocState) => void;
    onnotice: (message: string) => void;
    onclose: () => void;
  }

  let { kind, docId, pageCount, pages, onstate, onnotice, onclose }: Props = $props();

  let busy = $state(false);
  let error = $state("");

  // Properties
  let info: Properties | null = $state(null);
  // Protection
  let protect = $state(false);
  let pw = $state("");
  let pw2 = $state("");
  let allowPrint = $state(true);
  let allowCopy = $state(true);
  let allowEdit = $state(false);
  let wasProtected = $state(false);
  // Compress
  let level: "light" | "medium" | "strong" = $state("medium");
  let report: CompressReport | null = $state(null);
  // Export
  let which: "selected" | "all" = $state("selected");
  let format: "png" | "jpeg" = $state("png");
  let dpi = $state(150);

  const LEVELS = {
    light: { dpi: 220, quality: 85, label: "Light — print quality (220 dpi images)" },
    medium: { dpi: 150, quality: 75, label: "Medium — screen and office printing (150 dpi)" },
    strong: { dpi: 96, quality: 60, label: "Strong — smallest file, screen only (96 dpi)" },
  };

  onMount(async () => {
    try {
      if (kind === "properties") info = await documentProperties(docId);
      if (kind === "protect") {
        const p = await documentProtection(docId);
        wasProtected = !!p?.userPassword;
        protect = wasProtected;
        if (p) {
          allowPrint = p.allowPrint;
          allowCopy = p.allowCopy;
          allowEdit = p.allowEdit;
        }
      }
    } catch (e) {
      error = String(e);
    }
  });

  async function act(fn: () => Promise<void>) {
    busy = true;
    error = "";
    try {
      await fn();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  const saveProps = () =>
    act(async () => {
      onstate(await setDocumentProperties(docId, info!));
      onclose();
    });

  const saveProtection = () =>
    act(async () => {
      if (protect && (!pw || pw !== pw2)) {
        error = !pw ? "Enter a password." : "The passwords don't match.";
        return;
      }
      const p: Protection | null = protect
        ? { userPassword: pw, ownerPassword: "", allowPrint, allowCopy, allowEdit }
        : null;
      onstate(await setDocumentProtection(docId, p));
      onnotice(protect ? "Password protection will be applied when you save." : "Password protection will be removed when you save.");
      onclose();
    });

  /** Percentage saved; one decimal near 100% so it never reads "100%". */
  function savedPct(r: CompressReport) {
    const p = Math.max(0, (1 - r.after / r.before) * 100);
    return p >= 99 ? p.toFixed(1) : Math.round(p).toString();
  }

  const runCompress = () =>
    act(async () => {
      const l = LEVELS[level];
      const r = await compressDocument(docId, l.dpi, l.quality);
      onstate(r.state);
      report = r.report;
    });

  const runExport = () =>
    act(async () => {
      const dir = await open({ directory: true, multiple: false, title: "Choose a folder for the images" });
      if (!dir) return;
      const list = which === "all" ? Array.from({ length: pageCount }, (_, i) => i) : pages;
      const files = await exportImages(docId, list, dpi, format, dir);
      onnotice(`Exported ${files.length} image${files.length > 1 ? "s" : ""} to ${dir}`);
      onclose();
    });
</script>

<div class="backdrop">
  <div class="dialog" role="dialog" aria-label={kind}>
    {#if kind === "properties"}
      <h2>Document properties</h2>
      {#if info}
        <div class="grid">
          <label for="p-title">Title</label><input id="p-title" bind:value={info.title} />
          <label for="p-author">Author</label><input id="p-author" bind:value={info.author} />
          <label for="p-subject">Subject</label><input id="p-subject" bind:value={info.subject} />
          <label for="p-keywords">Keywords</label><input id="p-keywords" bind:value={info.keywords} />
        </div>
        <dl class="facts">
          <dt>Pages</dt><dd>{info.pageCount} · {info.pageSize}</dd>
          <dt>File size</dt><dd>{formatBytes(info.fileSize)}</dd>
          <dt>PDF version</dt><dd>{info.version || "—"}</dd>
          <dt>Created</dt><dd>{info.created || "—"}</dd>
          <dt>Modified</dt><dd>{info.modified || "—"}</dd>
          <dt>Application</dt><dd>{info.creator || "—"}</dd>
          <dt>Producer</dt><dd>{info.producer || "—"}</dd>
          <dt>Password</dt><dd>{info.protected ? "Required to open" : "None"}</dd>
        </dl>
      {:else if !error}<p class="muted">Loading…</p>{/if}
      <div class="actions">
        <button onclick={onclose}>Cancel</button>
        <button class="primary" disabled={!info || busy} onclick={saveProps}>Apply</button>
      </div>
    {:else if kind === "protect"}
      <h2>Password protection</h2>
      <label class="check"><input type="checkbox" bind:checked={protect} /> Require a password to open this document</label>
      {#if protect}
        <div class="grid">
          <label for="pw1">Password</label><input id="pw1" type="password" bind:value={pw} autocomplete="new-password" />
          <label for="pw2">Repeat</label><input id="pw2" type="password" bind:value={pw2} autocomplete="new-password" />
        </div>
        <fieldset>
          <legend class="muted">People who open it may</legend>
          <label class="check"><input type="checkbox" bind:checked={allowPrint} /> Print</label>
          <label class="check"><input type="checkbox" bind:checked={allowCopy} /> Copy text and images</label>
          <label class="check"><input type="checkbox" bind:checked={allowEdit} /> Edit, comment and fill forms</label>
        </fieldset>
        <p class="muted small">Encrypted with AES-256. There is no way to recover a forgotten password.</p>
      {:else if wasProtected}
        <p class="muted small">The password will be removed when you save.</p>
      {/if}
      <div class="actions">
        <button onclick={onclose}>Cancel</button>
        <button class="primary" disabled={busy} onclick={saveProtection}>Apply</button>
      </div>
    {:else if kind === "compress"}
      <h2>Reduce file size</h2>
      {#if report}
        <p>
          {formatBytes(report.before)} → <b>{formatBytes(report.after)}</b>
          ({savedPct(report)}% smaller)
        </p>
        <p class="muted small">
          {report.imagesResampled ? `${report.imagesResampled} image${report.imagesResampled > 1 ? "s" : ""} resampled. ` : "No images needed resampling. "}
          Save the document to keep the result, or undo (Ctrl+Z) to revert.
        </p>
        <div class="actions"><button class="primary" onclick={onclose}>Done</button></div>
      {:else}
        {#each Object.entries(LEVELS) as [key, l]}
          <label class="check"><input type="radio" name="level" value={key} bind:group={level} /> {l.label}</label>
        {/each}
        <p class="muted small">Large images are resampled to JPEG and unused data is removed. Text and vector graphics are not changed.</p>
        <div class="actions">
          <button onclick={onclose}>Cancel</button>
          <button class="primary" disabled={busy} onclick={runCompress}>{busy ? "Compressing…" : "Compress"}</button>
        </div>
      {/if}
    {:else}
      <h2>Export as images</h2>
      <label class="check"><input type="radio" value="selected" bind:group={which} /> Page{pages.length > 1 ? "s" : ""} {pageLabel(pages)}</label>
      <label class="check"><input type="radio" value="all" bind:group={which} /> All {pageCount} pages</label>
      <div class="grid">
        <label for="fmt">Format</label>
        <select id="fmt" bind:value={format}>
          <option value="png">PNG (sharp, larger)</option>
          <option value="jpeg">JPEG (smaller)</option>
        </select>
        <label for="dpi">Resolution</label>
        <select id="dpi" bind:value={dpi}>
          <option value={72}>72 dpi (screen)</option>
          <option value={150}>150 dpi</option>
          <option value={300}>300 dpi (print)</option>
        </select>
      </div>
      <div class="actions">
        <button onclick={onclose}>Cancel</button>
        <button class="primary" disabled={busy} onclick={runExport}>{busy ? "Exporting…" : "Choose folder…"}</button>
      </div>
    {/if}
    {#if error}<p class="bad">{error}</p>{/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: center;
    z-index: 10;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 18px 20px;
    width: min(460px, calc(100vw - 32px));
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 16px;
  }
  .grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 10px;
    align-items: center;
  }
  .facts {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 6px 0 0;
    font-size: 13px;
  }
  .facts dt {
    color: var(--muted);
  }
  .facts dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .check input {
    min-height: 0;
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  p {
    margin: 0;
  }
  .bad {
    color: var(--danger);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
</style>
