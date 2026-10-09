<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ask, message, open, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import PdfView from "$lib/PdfView.svelte";
  import Thumbnails from "$lib/Thumbnails.svelte";
  import Outline from "$lib/Outline.svelte";
  import Icon from "$lib/Icon.svelte";
  import type { Tool } from "$lib/EditLayer.svelte";
  import type { AnnotTool, Signature } from "$lib/AnnotLayer.svelte";
  import SignatureDialog from "$lib/SignatureDialog.svelte";
  import DocDialogs, { type DocDialog } from "$lib/DocDialogs.svelte";
  import AboutDialog from "$lib/AboutDialog.svelte";
  import ProMoreMenu from "$pro/MoreMenu.svelte";
  import ProHost from "$pro/Host.svelte";
  import ProStart from "$pro/Start.svelte";
  import { capabilities as loadCapabilities, type ExtensionContext } from "$lib/extension";
  import { check, type Update } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
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
    saveDocument,
    undo as undoApi,
    redo as redoApi,
    type DocInfo,
    type DocState,
    type NewTextStyle,
    hexToRgb,
    rotatePages,
    deletePages,
    movePages,
    insertBlankPage,
    insertPagesFromFile,
    insertImagePage,
    extractPages,
    splitDocument,
    pageLabel,
    IMAGE_FILTERS,
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
    revision: number;
    dirty: boolean;
    canUndo: boolean;
    canRedo: boolean;
    /** Bumped when the page list changes, to rebuild the views. */
    structure: number;
    /** Pages selected in the thumbnail panel. */
    pageSel: number[];
  }

  let tabs: Tab[] = $state([]);
  let activeKey: number | null = $state(null);
  let nextKey = 1;
  const views: Record<number, PdfView> = $state({});
  const tab = $derived(tabs.find((t) => t.key === activeKey) ?? null);
  const view = $derived(tab ? views[tab.key] : undefined);

  let error = $state("");
  let notice = $state("");
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  // Editing
  let tool: Tool | null = $state(null);
  let newTextStyle: NewTextStyle = $state({ size: 12, family: "sans-serif", bold: false, italic: false, color: [0, 0, 0] });
  let newTextColor = $state("#000000");
  const anyDirty = $derived(tabs.some((t) => t.dirty));

  // Comments (annotations)
  let annotTool: AnnotTool | null = $state(null);
  const COLORS = ["#ffd400", "#7ad151", "#4fc3f7", "#ff8ac2", "#e0262f", "#1e5eff", "#1a1a1a"];
  let toolColors: Record<string, string> = $state({
    highlight: "#ffd400",
    underline: "#1e5eff",
    strikeout: "#e0262f",
    note: "#ffd400",
    pen: "#e0262f",
    rectangle: "#1e5eff",
    ellipse: "#1e5eff",
  });
  let annotWidth = $state(2);
  let signature: Signature | null = $state(null);
  let signatureDialog = $state(false);
  const annotColor = $derived(hexToRgb(toolColors[annotTool ?? ""] ?? "#ffd400"));

  function setMode(mode: "edit" | "comment" | null) {
    tool = mode === "edit" ? "edit" : null;
    annotTool = mode === "comment" ? "select" : null;
  }

  async function pickAnnotTool(t: AnnotTool) {
    if (t === "signature") {
      signatureDialog = true;
      return;
    }
    annotTool = t;
    // Like Acrobat: text already selected gets marked up right away.
    if ((t === "highlight" || t === "underline" || t === "strikeout") && view?.hasSelection()) {
      await view.applyMarkup(t, hexToRgb(toolColors[t]));
    }
  }

  const ANNOT_TOOLS: { id: AnnotTool; icon: import("$lib/Icon.svelte").IconName; label: string }[] = [
    { id: "select", icon: "pointer", label: "Select" },
    { id: "highlight", icon: "highlight", label: "Highlight" },
    { id: "underline", icon: "underline", label: "Underline" },
    { id: "strikeout", icon: "strike", label: "Strikethrough" },
    { id: "note", icon: "note", label: "Note" },
    { id: "pen", icon: "pen", label: "Draw" },
    { id: "rectangle", icon: "square", label: "Rectangle" },
    { id: "ellipse", icon: "circle", label: "Ellipse" },
    { id: "signature", icon: "signature", label: "Sign" },
  ];
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
        revision: 0,
        dirty: false,
        canUndo: false,
        canRedo: false,
        structure: 0,
        pageSel: [],
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

  function showNotice(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ""), 8000);
  }

  /** Applies the result of a document change to its tab. */
  function applyState(t: Tab, s: DocState) {
    t.revision = s.revision;
    t.dirty = s.dirty;
    t.canUndo = s.canUndo;
    t.canRedo = s.canRedo;
    if (s.path !== t.doc.path) t.doc = { ...t.doc, path: s.path };
    if (s.pages) {
      const old = t.doc.pages;
      const same =
        old.length === s.pages.length &&
        old.every((p, i) => p.width === s.pages![i].width && p.height === s.pages![i].height);
      if (!same) {
        t.doc = { ...t.doc, pages: s.pages };
        t.currentPage = Math.min(t.currentPage, s.pages.length - 1);
        t.pageSel = t.pageSel.filter((p) => p < s.pages!.length);
        t.structure++;
      }
    }
    // Search results point into the old text.
    t.hits = [];
    t.activeHit = -1;
    t.searchedQuery = "";
    if (s.substituted) {
      showNotice(
        `The document's embedded font has no glyphs for “${s.substituted}”, so a similar font was used for those characters.`,
      );
    }
  }

  async function changeDoc(t: Tab, op: () => Promise<DocState>) {
    try {
      applyState(t, await op());
    } catch (e) {
      error = String(e);
    }
  }

  async function saveTab(t: Tab, as = false): Promise<boolean> {
    let path: string | undefined;
    if (as) {
      const picked = await saveDialog({ defaultPath: t.doc.path, filters: [{ name: "PDF", extensions: ["pdf"] }] });
      if (!picked) return false;
      path = picked.toLowerCase().endsWith(".pdf") ? picked : `${picked}.pdf`;
    }
    try {
      applyState(t, await saveDocument(t.doc.id, path));
      recent = rememberRecent(t.doc.path);
      showNotice(`Saved ${fileName(t.doc.path)}`);
      return true;
    } catch (e) {
      error = `Could not save: ${e}`;
      return false;
    }
  }

  /** Asks about unsaved changes; returns false if the user cancelled. */
  async function confirmDiscard(t: Tab): Promise<boolean> {
    if (!t.dirty) return true;
    const answer = await message(`Save changes to ${fileName(t.doc.path)} before closing?`, {
      title: APP_NAME,
      kind: "warning",
      buttons: { yes: "Save", no: "Don't save", cancel: "Cancel" },
    });
    if (answer === "Yes" || answer === "Save") return saveTab(t);
    return answer === "No" || answer === "Don't save";
  }

  async function closeTab(key: number) {
    const i0 = tabs.findIndex((t) => t.key === key);
    if (i0 < 0) return;
    activeKey = key;
    if (!(await confirmDiscard(tabs[i0]))) return;
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

  // ---- Updates ----

  let update: Update | null = $state(null);
  let caps: string[] = $state([]);
  let aboutOpen = $state(false);
  let updating = $state(false);

  async function checkForUpdate() {
    try {
      update = await check();
    } catch {
      // Offline, no release yet, or a dev build: stay quiet.
    }
  }

  async function installUpdate() {
    if (!update) return;
    if (anyDirty) {
      const ok = await ask("You have unsaved changes. Install the update and restart anyway?", {
        title: APP_NAME,
        kind: "warning",
        okLabel: "Restart",
        cancelLabel: "Cancel",
      });
      if (!ok) return;
    }
    updating = true;
    try {
      await update.downloadAndInstall();
      await relaunch();
    } catch (e) {
      error = `Update failed: ${e}`;
      updating = false;
    }
  }

  // ---- Extensions ($pro slot) ----

  const extCtx: ExtensionContext = $derived({
    doc: tab
      ? {
          id: tab.doc.id,
          path: tab.doc.path,
          pageCount: tab.doc.pages.length,
          currentPage: tab.currentPage,
          pages: tab.pageSel.length ? [...tab.pageSel].sort((a, b) => a - b) : [tab.currentPage],
          sizes: tab.doc.pages,
        }
      : null,
    capabilities: caps,
    onstate: (s) => {
      if (tab) applyState(tab, s);
    },
    onnotice: (m) => showNotice(m),
    onerror: (m) => (error = m),
    reloadCapabilities: async () => {
      caps = await loadCapabilities();
    },
  });

  // ---- Document dialogs, More menu, redaction ----

  let docDialog: DocDialog | null = $state(null);
  let moreOpen = $state(false);
  let redactMarks = $state(0);

  async function applyRedactions() {
    const n = redactMarks;
    const ok = await ask(
      `Permanently remove the text, images and comments under ${n} marked area${n > 1 ? "s" : ""}? ` +
        "Undo works until you save; after saving the content is gone for good.",
      { title: APP_NAME, kind: "warning", okLabel: "Redact", cancelLabel: "Cancel" },
    );
    if (ok && (await view?.applyRedactions())) showNotice("Redacted. Save to make it permanent.");
  }

  // ---- Pages ----

  let splitFor: Tab | null = $state(null);
  let splitEvery = $state(1);

  const targetPages = (t: Tab) => (t.pageSel.length ? [...t.pageSel].sort((a, b) => a - b) : [t.currentPage]);

  async function deleteSelectedPages(t: Tab, pages = targetPages(t)) {
    if (pages.length >= t.doc.pages.length) {
      error = "A document must keep at least one page.";
      return;
    }
    const ok = await ask(`Delete page${pages.length > 1 ? "s" : ""} ${pageLabel(pages)}? You can undo this.`, {
      title: APP_NAME,
      kind: "warning",
      okLabel: "Delete",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    t.pageSel = [];
    await changeDoc(t, () => deletePages(t.doc.id, pages));
  }

  async function insertPages(t: Tab, kind: "blank" | "file" | "image") {
    const sel = targetPages(t);
    const at = sel[sel.length - 1] + 1;
    if (kind === "blank") {
      const ref = t.doc.pages[at - 1];
      await changeDoc(t, () => insertBlankPage(t.doc.id, at, ref.width, ref.height));
    } else if (kind === "file") {
      const path = await open({ multiple: false, directory: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
      if (path) await changeDoc(t, () => insertPagesFromFile(t.doc.id, at, path));
    } else {
      const path = await open({ multiple: false, directory: false, filters: IMAGE_FILTERS });
      if (path) await changeDoc(t, () => insertImagePage(t.doc.id, at, path));
    }
    t.pageSel = [at];
    views[t.key]?.goToPage(at);
  }

  async function extractSelected(t: Tab) {
    const pages = targetPages(t);
    const stem = fileName(t.doc.path).replace(/\.pdf$/i, "");
    const out = await saveDialog({
      defaultPath: t.doc.path.replace(/[^\\/]*$/, `${stem} (pages ${pageLabel(pages).replace(/, /g, ",")}).pdf`),
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (!out) return;
    try {
      await extractPages(t.doc.id, pages, out.toLowerCase().endsWith(".pdf") ? out : `${out}.pdf`);
      showNotice(`Saved page${pages.length > 1 ? "s" : ""} ${pageLabel(pages)} to ${fileName(out)}`);
    } catch (e) {
      error = `Could not extract pages: ${e}`;
    }
  }

  async function runSplit() {
    const t = splitFor;
    if (!t) return;
    splitFor = null;
    const dir = await open({ directory: true, multiple: false, title: "Choose a folder for the split files" });
    if (!dir) return;
    try {
      const files = await splitDocument(t.doc.id, Math.max(1, Math.floor(splitEvery)), dir);
      showNotice(`Created ${files.length} files in ${dir}`);
    } catch (e) {
      error = `Could not split: ${e}`;
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
    if ((tool || annotTool) && !typing && !mod && view?.handleEditKey(e)) {
      e.preventDefault();
      return;
    }
    if (annotTool && annotTool !== "select" && e.key === "Escape" && !typing) {
      annotTool = "select";
      return;
    }
    if (tab && mod && !typing) {
      const k = e.key.toLowerCase();
      const t = tab;
      if (k === "z" && !e.shiftKey) {
        e.preventDefault();
        if (t.canUndo) changeDoc(t, () => undoApi(t.doc.id));
        return;
      }
      if (k === "y" || (k === "z" && e.shiftKey)) {
        e.preventDefault();
        if (t.canRedo) changeDoc(t, () => redoApi(t.doc.id));
        return;
      }
    }
    if (tab && mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      saveTab(tab, e.shiftKey);
      return;
    }
    if (tab && mod && e.key.toLowerCase() === "e") {
      e.preventDefault();
      setMode(tool ? null : "edit");
      return;
    }
    if (aboutOpen && e.key === "Escape") {
      aboutOpen = false;
      return;
    }
    if (e.key === "F1") {
      e.preventDefault();
      aboutOpen = true;
      return;
    }
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
    else if (k === "c" && !typing && !tool) view?.copySelection();
    else if (k === "a" && !typing && !tool) view?.selectAll();
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

    // Ask before closing the window with unsaved changes.
    const appWindow = getCurrentWindow();
    const unlistenClose = appWindow.onCloseRequested(async (event) => {
      const dirty = tabs.filter((t) => t.dirty);
      if (!dirty.length) return;
      event.preventDefault();
      const names = dirty.map((t) => fileName(t.doc.path)).join(", ");
      const answer = await message(`Save changes before closing?\n\n${names}`, {
        title: APP_NAME,
        kind: "warning",
        buttons: { yes: "Save all", no: "Don't save", cancel: "Cancel" },
      });
      if (answer === "Yes" || answer === "Save all") {
        for (const t of dirty) if (!(await saveTab(t))) return;
      } else if (!(answer === "No" || answer === "Don't save")) {
        return;
      }
      await appWindow.destroy();
    });

    loadCapabilities().then((c) => (caps = c));

    // Release builds look for a newer version on GitHub a few seconds after start.
    if (import.meta.env.PROD) setTimeout(checkForUpdate, 4000);

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
      unlistenClose.then((f) => f());
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
            {#if t.dirty}<span class="dirty" title="Unsaved changes">●</span>{/if}{fileName(t.doc.path)}
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
      <button class="icon-btn" onclick={() => saveTab(tab)} disabled={!tab.dirty} title="Save (Ctrl+S)" aria-label="Save">
        <Icon name="save" />
      </button>
      <button class="icon-btn" onclick={print} disabled={!!printing} title="Print (Ctrl+P)" aria-label="Print">
        <Icon name="print" />
      </button>
      <div class="menu-wrap">
        <button class="icon-btn" class:on={moreOpen} onclick={() => (moreOpen = !moreOpen)} title="More" aria-label="More" aria-haspopup="menu" aria-expanded={moreOpen}>
          <Icon name="more" />
        </button>
        {#if moreOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
          <div class="menu-scrim" onclick={() => (moreOpen = false)}></div>
          <div class="menu open more-menu" role="menu">
            {#each [
              { label: "Save as…", icon: "save", act: () => saveTab(tab, true) },
              { label: "Document properties…", icon: "info", act: () => (docDialog = "properties") },
              { label: "Password protection…", icon: "lock", act: () => (docDialog = "protect") },
              { label: "Reduce file size…", icon: "shrink", act: () => (docDialog = "compress") },
              { label: "Export as images…", icon: "export", act: () => (docDialog = "export") },
            ] as item}
              <button role="menuitem" onclick={() => { moreOpen = false; item.act(); }}>
                <Icon name={item.icon as import("$lib/Icon.svelte").IconName} size={16} />{item.label}
              </button>
            {/each}
            <ProMoreMenu ctx={extCtx} close={() => (moreOpen = false)} />
          </div>
        {/if}
      </div>
      <span class="sep"></span>
      <button class="edit-toggle" class:on={!!tool} onclick={() => setMode(tool ? null : "edit")} title="Edit text and images (Ctrl+E)">
        <Icon name="edit" size={16} />Edit
      </button>
      <button class="edit-toggle" class:on={!!annotTool} onclick={() => setMode(annotTool ? null : "comment")} title="Comment, highlight, draw and sign">
        <Icon name="comment" size={16} />Comment
      </button>
      <button class="icon-btn" onclick={() => changeDoc(tab, () => undoApi(tab.doc.id))} disabled={!tab.canUndo} title="Undo (Ctrl+Z)" aria-label="Undo">
        <Icon name="undo" />
      </button>
      <button class="icon-btn" onclick={() => changeDoc(tab, () => redoApi(tab.doc.id))} disabled={!tab.canRedo} title="Redo (Ctrl+Y)" aria-label="Redo">
        <Icon name="redo" />
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
    {:else}
      <span class="spacer"></span>
    {/if}
    <button class="icon-btn" onclick={() => (aboutOpen = true)} title="About (F1)" aria-label="About Yet Another PDF Editor">
      <Icon name="info" />
    </button>
  </header>

  {#if tab && tool}
    <div class="edit-bar" role="toolbar" aria-label="Edit tools">
      <button class="tool" class:on={tool === "edit"} onclick={() => (tool = "edit")} title="Select, move and edit text and images">
        <Icon name="pointer" size={16} />Select
      </button>
      <button class="tool" class:on={tool === "addText"} onclick={() => (tool = "addText")} title="Click on the page to add text">
        <Icon name="text" size={16} />Add text
      </button>
      <button class="tool" class:on={tool === "addImage"} onclick={() => (tool = "addImage")} title="Click on the page to add an image">
        <Icon name="image" size={16} />Add image
      </button>
      <button class="tool" class:on={tool === "redact"} onclick={() => (tool = "redact")} title="Mark areas to remove permanently">
        <Icon name="redact" size={16} />Redact
      </button>
      {#if redactMarks}
        <button class="danger-btn" onclick={applyRedactions}>Apply {redactMarks} redaction{redactMarks > 1 ? "s" : ""}</button>
        <button class="link" onclick={() => view?.clearRedactions()}>Clear marks</button>
      {/if}
      {#if tool === "addText"}
        <span class="sep"></span>
        <select bind:value={newTextStyle.family} aria-label="Font">
          <option value="sans-serif">Sans</option>
          <option value="serif">Serif</option>
          <option value="monospace">Mono</option>
        </select>
        <input class="size" type="number" min="4" max="144" bind:value={newTextStyle.size} aria-label="Font size" />
        <button class="icon-btn style" class:on={newTextStyle.bold} onclick={() => (newTextStyle.bold = !newTextStyle.bold)} aria-label="Bold"><b>B</b></button>
        <button class="icon-btn style" class:on={newTextStyle.italic} onclick={() => (newTextStyle.italic = !newTextStyle.italic)} aria-label="Italic"><i>I</i></button>
        <input
          class="color"
          type="color"
          bind:value={newTextColor}
          oninput={() => {
            const n = parseInt(newTextColor.slice(1), 16);
            newTextStyle.color = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
          }}
          aria-label="Text colour"
        />
      {/if}
      <span class="hint muted">
        {#if tool === "edit"}Double-click text to edit · drag to move · corners resize images · Del deletes
        {:else if tool === "addText"}Click where the text should start · Ctrl+Enter or click outside to finish
        {:else if tool === "redact"}Drag over anything to remove · then Apply
        {:else}Click where the image's top-left corner should go{/if}
      </span>
    </div>
  {/if}

  {#if tab && annotTool}
    <div class="edit-bar" role="toolbar" aria-label="Comment tools">
      {#each ANNOT_TOOLS as t}
        <button class="tool" class:on={annotTool === t.id} onclick={() => pickAnnotTool(t.id)} title={t.label}>
          <Icon name={t.icon} size={16} /><span class="tool-label">{t.label}</span>
        </button>
      {/each}
      {#if annotTool !== "select" && annotTool !== "signature"}
        <span class="sep"></span>
        {#each COLORS as c}
          <button
            class="swatch"
            class:on={toolColors[annotTool] === c}
            style:background={c}
            onclick={() => (toolColors[annotTool!] = c)}
            aria-label="Colour {c}"
          ></button>
        {/each}
        {#if annotTool === "pen" || annotTool === "rectangle" || annotTool === "ellipse"}
          <select bind:value={annotWidth} aria-label="Line width">
            <option value={1}>Thin</option>
            <option value={2}>Medium</option>
            <option value={4}>Thick</option>
          </select>
        {/if}
      {/if}
      <span class="hint muted">
        {#if annotTool === "select"}Click a comment to select · drag to move · Del deletes · double-click a note to edit
        {:else if annotTool === "highlight" || annotTool === "underline" || annotTool === "strikeout"}Drag across text
        {:else if annotTool === "note"}Click where the note should go
        {:else if annotTool === "pen"}Draw on the page · strokes made together become one drawing
        {:else if annotTool === "signature"}Click where the signature should go · Esc to cancel
        {:else}Drag on the page to draw{/if}
      </span>
    </div>
  {/if}

  {#if update}
    <div class="notice" role="status">
      Version {update.version} is available.
      <button class="primary small-btn" disabled={updating} onclick={installUpdate}>
        {updating ? "Installing…" : "Install and restart"}
      </button>
      <button class="link" onclick={() => (update = null)}>Later</button>
    </div>
  {/if}

  {#if notice}
    <div class="notice" role="status">
      {notice}
      <button class="link" onclick={() => (notice = "")}>Dismiss</button>
    </div>
  {/if}

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
              {@const t = tab}
              <div class="page-tools" role="toolbar" aria-label="Page tools">
                <button class="icon-btn small" title="Rotate left" aria-label="Rotate left" onclick={() => changeDoc(t, () => rotatePages(t.doc.id, targetPages(t), -90))}><Icon name="rotateLeft" size={16} /></button>
                <button class="icon-btn small" title="Rotate right" aria-label="Rotate right" onclick={() => changeDoc(t, () => rotatePages(t.doc.id, targetPages(t), 90))}><Icon name="rotateRight" size={16} /></button>
                <button class="icon-btn small" title="Delete pages (Del)" aria-label="Delete pages" onclick={() => deleteSelectedPages(t)}><Icon name="trash" size={16} /></button>
                <div class="menu-wrap">
                  <button class="icon-btn small" title="Insert pages" aria-label="Insert pages" aria-haspopup="menu" onclick={(e) => e.currentTarget.nextElementSibling?.classList.toggle("open")}><Icon name="insert" size={16} /></button>
                  <div class="menu" role="menu">
                    <button role="menuitem" onclick={(e) => { e.currentTarget.parentElement?.classList.remove("open"); insertPages(t, "blank"); }}>Blank page</button>
                    <button role="menuitem" onclick={(e) => { e.currentTarget.parentElement?.classList.remove("open"); insertPages(t, "file"); }}>Pages from PDF…</button>
                    <button role="menuitem" onclick={(e) => { e.currentTarget.parentElement?.classList.remove("open"); insertPages(t, "image"); }}>Image as page…</button>
                  </div>
                </div>
                <button class="icon-btn small" title="Extract pages to a new PDF" aria-label="Extract pages" onclick={() => extractSelected(t)}><Icon name="extract" size={16} /></button>
                <button class="icon-btn small" title="Split into several PDFs" aria-label="Split document" onclick={() => { splitEvery = Math.max(1, Math.ceil(t.doc.pages.length / 2)); splitFor = t; }}><Icon name="scissors" size={16} /></button>
              </div>
              {#if t.pageSel.length > 1}
                <p class="sel-info muted">{t.pageSel.length} pages selected</p>
              {/if}
              {#key t.structure}
                <Thumbnails
                  doc={t.doc}
                  currentPage={t.currentPage}
                  revision={t.revision}
                  selected={t.pageSel}
                  onselect={(p) => view?.goToPage(p)}
                  onselectionchange={(pages) => (t.pageSel = pages)}
                  onmove={(pages, before) => changeDoc(t, () => movePages(t.doc.id, pages, before))}
                  ondelete={(pages) => deleteSelectedPages(t, pages)}
                />
              {/key}
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
          {#key t.structure}
          <PdfView
            doc={t.doc}
            bind:zoom={t.zoom}
            bind:currentPage={t.currentPage}
            searchHits={t.hits}
            activeHit={t.activeHit}
            tool={t.key === activeKey ? tool : null}
            revision={t.revision}
            {newTextStyle}
            onstate={(s) => applyState(t, s)}
            onerror={(m) => (error = m)}
            ontooldone={() => (tool = "edit")}
            annotTool={t.key === activeKey ? annotTool : null}
            {annotColor}
            {annotWidth}
            {signature}
            onannotdone={() => (annotTool = "select")}
            onmarkschange={(n) => {
              if (t.key === activeKey) redactMarks = n;
            }}
            bind:this={views[t.key]}
          />
          {/key}
        </div>
      {/each}
      {#if !tab}
        <div class="empty">
          <img src="/app-icon.png" alt="" width="96" height="96" />
          <h1>{APP_NAME}</h1>
          <button class="primary" onclick={pickFile}>Open file…</button>
          <p class="muted">or drop PDFs anywhere in this window</p>
          <ProStart ctx={extCtx} />
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

{#if docDialog && tab}
  {@const t = tab}
  <DocDialogs
    kind={docDialog}
    docId={t.doc.id}
    pageCount={t.doc.pages.length}
    pages={targetPages(t)}
    onstate={(s) => applyState(t, s)}
    onnotice={showNotice}
    onclose={() => (docDialog = null)}
  />
{/if}

<ProHost ctx={extCtx} />

{#if aboutOpen}
  <AboutDialog
    edition={caps.includes("pro") ? "Pro" : "Community"}
    oncheck={async () => {
      update = await check();
      return update?.version ?? null;
    }}
    oninstall={() => {
      aboutOpen = false;
      installUpdate();
    }}
    onclose={() => (aboutOpen = false)}
  />
{/if}

{#if splitFor}
  <div class="backdrop">
    <form
      class="dialog"
      onsubmit={(e) => {
        e.preventDefault();
        runSplit();
      }}
    >
      <h2>Split document</h2>
      <label class="split-row">
        Every
        <input type="number" min="1" max={splitFor.doc.pages.length} bind:value={splitEvery} aria-label="Pages per file" />
        pages
      </label>
      <p class="muted">
        {Math.ceil(splitFor.doc.pages.length / Math.max(1, Math.floor(splitEvery) || 1))} files from {splitFor.doc.pages.length} pages.
        The original stays unchanged.
      </p>
      <div class="actions">
        <button type="button" onclick={() => (splitFor = null)}>Cancel</button>
        <button type="submit" class="primary">Choose folder…</button>
      </div>
    </form>
  </div>
{/if}

{#if signatureDialog}
  <SignatureDialog
    onpick={(sig) => {
      signature = sig;
      signatureDialog = false;
      tool = null;
      annotTool = "signature";
    }}
    oncancel={() => (signatureDialog = false)}
  />
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
  /* Pinned to the viewport so focusing or scrolling inside can never
     scroll the window itself. */
  .app {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
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
  .dirty {
    color: var(--accent);
    font-size: 10px;
    margin-right: 6px;
    vertical-align: 1px;
  }
  .edit-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border-color: transparent;
    background: transparent;
  }
  .edit-toggle.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 40%, transparent);
  }
  .edit-bar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--accent) 6%, var(--bg));
    flex: none;
    overflow: hidden;
  }
  .tool {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border-color: transparent;
    background: transparent;
    white-space: nowrap;
  }
  .tool.on {
    background: var(--btn);
    border-color: var(--border);
    color: var(--accent);
  }
  .swatch {
    width: 20px;
    height: 20px;
    min-height: 0;
    padding: 0;
    border-radius: 50%;
    border: 2px solid var(--bg);
    box-shadow: 0 0 0 1px var(--border);
  }
  .swatch.on {
    box-shadow: 0 0 0 2px var(--accent);
  }
  @media (max-width: 1400px) {
    .tool-label {
      display: none;
    }
  }
  .edit-bar .size {
    width: 4.2em;
  }
  .edit-bar .color {
    width: 34px;
    padding: 2px;
  }
  .icon-btn.style {
    font-family: Georgia, serif;
    font-size: 15px;
  }
  .hint {
    margin-left: 10px;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .notice {
    padding: 8px 12px;
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
    border-bottom: 1px solid var(--border);
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
  .menu-scrim {
    position: fixed;
    inset: 0;
    z-index: 4;
  }
  .more-menu {
    min-width: 220px;
  }
  .more-menu button {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .small-btn {
    min-height: 26px;
    padding: 2px 10px;
    margin: 0 6px;
  }
  .danger-btn {
    margin-left: 8px;
    background: #e0262f;
    border-color: #e0262f;
    color: #fff;
    white-space: nowrap;
  }
  .danger-btn:hover:not(:disabled) {
    background: #c81f28;
  }
  .page-tools {
    display: flex;
    justify-content: center;
    gap: 2px;
    padding: 4px;
    border-bottom: 1px solid var(--border);
  }
  .menu-wrap {
    position: relative;
  }
  .menu {
    display: none;
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 5;
    min-width: 170px;
    padding: 4px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.2);
  }
  :global(.menu.open) {
    display: flex;
    flex-direction: column;
  }
  .menu button {
    border: none;
    background: transparent;
    text-align: left;
  }
  .menu button:hover {
    background: var(--btn-hover);
  }
  .sel-info {
    margin: 4px 0 0;
    text-align: center;
    font-size: 12px;
  }
  .split-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .split-row input {
    width: 5em;
  }
  .side-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .outline-scroll {
    flex: 1;
    min-height: 0;
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
