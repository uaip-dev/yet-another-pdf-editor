<script lang="ts">
  import { onMount, tick } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { IMAGE_FILTERS, readImageFile } from "./api";
  import type { Signature } from "./AnnotLayer.svelte";

  interface Props {
    onpick: (sig: Signature) => void;
    oncancel: () => void;
  }

  let { onpick, oncancel }: Props = $props();

  const SAVED_KEY = "signatures";
  const SAVED_MAX = 5;
  const FONTS = ["Segoe Script", "Lucida Handwriting", "Ink Free", "Brush Script MT", "cursive"];
  const INKS = ["#1a1a1a", "#1b3fa0", "#0b6e4f"];

  let mode: "draw" | "type" | "image" = $state("draw");
  let ink = $state(INKS[0]);
  let saved: string[] = $state(loadSaved());
  let remember = $state(true);

  // Draw
  let pad: HTMLCanvasElement | undefined = $state();
  let drawn = $state(false);

  // Type
  let typed = $state("");
  let font = $state(FONTS[0]);

  // Image
  let imageUrl = $state("");
  let imageEl: HTMLImageElement | undefined = $state();
  let removeWhite = $state(true);
  let error = $state("");

  function loadSaved(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(SAVED_KEY) ?? "[]");
      return Array.isArray(v) ? v.filter((s) => typeof s === "string").slice(0, SAVED_MAX) : [];
    } catch {
      return [];
    }
  }

  function store(list: string[]) {
    saved = list;
    try {
      localStorage.setItem(SAVED_KEY, JSON.stringify(list));
    } catch {
      /* not persisted */
    }
  }

  // ---- Drawing pad ----

  function setupPad() {
    if (!pad) return;
    const r = pad.getBoundingClientRect();
    pad.width = Math.round(r.width * devicePixelRatio);
    pad.height = Math.round(r.height * devicePixelRatio);
    drawn = false;
  }

  function onPadDown(e: PointerEvent) {
    if (!pad) return;
    const ctx = pad.getContext("2d")!;
    const k = devicePixelRatio;
    const r = pad.getBoundingClientRect();
    const pt = (ev: PointerEvent) => [(ev.clientX - r.left) * k, (ev.clientY - r.top) * k] as const;
    let [px, py] = pt(e);
    let [mx, my] = [px, py];
    ctx.strokeStyle = ink;
    ctx.lineWidth = 2.6 * k;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.beginPath();
    ctx.moveTo(px, py);
    ctx.lineTo(px + 0.1, py);
    ctx.stroke();
    pad.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const [x, y] = pt(ev);
      // Smooth: curve through midpoints.
      const [nx, ny] = [(px + x) / 2, (py + y) / 2];
      ctx.beginPath();
      ctx.moveTo(mx, my);
      ctx.quadraticCurveTo(px, py, nx, ny);
      ctx.stroke();
      [px, py, mx, my] = [x, y, nx, ny];
      drawn = true;
    };
    const up = () => {
      pad!.removeEventListener("pointermove", move);
      pad!.removeEventListener("pointerup", up);
    };
    pad.addEventListener("pointermove", move);
    pad.addEventListener("pointerup", up);
  }

  // ---- Export helpers ----

  /** Crops a canvas to its non-transparent pixels (plus a small margin). */
  function trim(src: HTMLCanvasElement): HTMLCanvasElement | null {
    const ctx = src.getContext("2d")!;
    const { width: w, height: h } = src;
    const data = ctx.getImageData(0, 0, w, h).data;
    let [x0, y0, x1, y1] = [w, h, -1, -1];
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        if (data[(y * w + x) * 4 + 3] > 8) {
          if (x < x0) x0 = x;
          if (x > x1) x1 = x;
          if (y < y0) y0 = y;
          if (y > y1) y1 = y;
        }
      }
    }
    if (x1 < 0) return null;
    const m = 4;
    [x0, y0, x1, y1] = [Math.max(0, x0 - m), Math.max(0, y0 - m), Math.min(w - 1, x1 + m), Math.min(h - 1, y1 + m)];
    const out = document.createElement("canvas");
    out.width = x1 - x0 + 1;
    out.height = y1 - y0 + 1;
    out.getContext("2d")!.drawImage(src, x0, y0, out.width, out.height, 0, 0, out.width, out.height);
    return out;
  }

  async function fromCanvas(c: HTMLCanvasElement): Promise<Signature> {
    const url = c.toDataURL("image/png");
    const blob = await new Promise<Blob>((r) => c.toBlob((b) => r(b!), "image/png"));
    return { kind: "png", png: new Uint8Array(await blob.arrayBuffer()), aspect: c.width / c.height, url };
  }

  async function fromDataUrl(url: string): Promise<Signature> {
    const img = new Image();
    img.src = url;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = img.naturalWidth;
    c.height = img.naturalHeight;
    c.getContext("2d")!.drawImage(img, 0, 0);
    return fromCanvas(c);
  }

  function typedCanvas(): HTMLCanvasElement | null {
    if (!typed.trim()) return null;
    const c = document.createElement("canvas");
    const ctx = c.getContext("2d")!;
    const size = 96;
    ctx.font = `${size}px "${font}", cursive`;
    const w = Math.ceil(ctx.measureText(typed).width) + size;
    c.width = w;
    c.height = size * 2;
    ctx.font = `${size}px "${font}", cursive`;
    ctx.fillStyle = ink;
    ctx.textBaseline = "middle";
    ctx.fillText(typed, size / 2, size);
    return trim(c);
  }

  function imageCanvas(): HTMLCanvasElement | null {
    if (!imageEl || !imageUrl) return null;
    const c = document.createElement("canvas");
    c.width = imageEl.naturalWidth;
    c.height = imageEl.naturalHeight;
    const ctx = c.getContext("2d")!;
    ctx.drawImage(imageEl, 0, 0);
    if (removeWhite) {
      // Scanned signatures: make near-white paper transparent.
      const img = ctx.getImageData(0, 0, c.width, c.height);
      const d = img.data;
      for (let i = 0; i < d.length; i += 4) {
        const light = Math.min(d[i], d[i + 1], d[i + 2]);
        if (light > 215) d[i + 3] = 0;
        else if (light > 170) d[i + 3] = Math.round(d[i + 3] * ((215 - light) / 45));
      }
      ctx.putImageData(img, 0, 0);
    }
    return trim(c);
  }

  async function pickImage() {
    error = "";
    const path = await open({ multiple: false, directory: false, filters: IMAGE_FILTERS });
    if (!path) return;
    try {
      if (imageUrl) URL.revokeObjectURL(imageUrl);
      imageUrl = URL.createObjectURL(await readImageFile(path));
    } catch (e) {
      error = String(e);
    }
  }

  async function use() {
    error = "";
    const c = mode === "draw" ? (drawn && pad ? trim(pad) : null) : mode === "type" ? typedCanvas() : imageCanvas();
    if (!c) {
      error = mode === "draw" ? "Draw your signature first." : mode === "type" ? "Type your name first." : "Choose an image first.";
      return;
    }
    const sig = await fromCanvas(c);
    if (remember && !saved.includes(sig.url)) store([sig.url, ...saved].slice(0, SAVED_MAX));
    onpick(sig);
  }

  $effect(() => {
    if (mode === "draw") tick().then(setupPad);
  });

  onMount(() => () => {
    if (imageUrl) URL.revokeObjectURL(imageUrl);
  });
</script>

<div class="backdrop">
  <div class="dialog" role="dialog" aria-label="Signature">
    <h2>Signature</h2>

    {#if saved.length}
      <div class="saved">
        <span class="muted">Saved</span>
        {#each saved as url, i}
          <div class="saved-item">
            <button class="saved-pick" onclick={async () => onpick(await fromDataUrl(url))} title="Use this signature">
              <img src={url} alt="Saved signature {i + 1}" />
            </button>
            <button class="x" title="Forget" aria-label="Forget signature" onclick={() => store(saved.filter((s) => s !== url))}>×</button>
          </div>
        {/each}
      </div>
    {/if}

    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={mode === "draw"} class:on={mode === "draw"} onclick={() => (mode = "draw")}>Draw</button>
      <button role="tab" aria-selected={mode === "type"} class:on={mode === "type"} onclick={() => (mode = "type")}>Type</button>
      <button role="tab" aria-selected={mode === "image"} class:on={mode === "image"} onclick={() => (mode = "image")}>Image</button>
      <span class="spacer"></span>
      {#if mode !== "image"}
        {#each INKS as c}
          <button class="ink" class:on={ink === c} style:background={c} onclick={() => (ink = c)} aria-label="Ink colour {c}"></button>
        {/each}
      {/if}
    </div>

    <div class="area">
      {#if mode === "draw"}
        <canvas bind:this={pad} class="pad" onpointerdown={onPadDown}></canvas>
        <button class="clear link" onclick={setupPad}>Clear</button>
        <span class="line"></span>
      {:else if mode === "type"}
        <input class="typed-input" bind:value={typed} placeholder="Type your name" aria-label="Your name" />
        <div class="fonts">
          {#each FONTS as f}
            <button class="font" class:on={font === f} style="font-family:'{f}',cursive;color:{ink}" onclick={() => (font = f)}>
              {typed || "Your Name"}
            </button>
          {/each}
        </div>
      {:else}
        <div class="image-pick">
          {#if imageUrl}
            <img bind:this={imageEl} src={imageUrl} alt="Chosen signature" />
          {/if}
          <button onclick={pickImage}>{imageUrl ? "Choose another…" : "Choose image…"}</button>
          <label><input type="checkbox" bind:checked={removeWhite} /> Make white background transparent</label>
        </div>
      {/if}
    </div>

    {#if error}<p class="bad">{error}</p>{/if}

    <div class="actions">
      <label class="remember"><input type="checkbox" bind:checked={remember} /> Remember on this computer</label>
      <span class="spacer"></span>
      <button onclick={oncancel}>Cancel</button>
      <button class="primary" onclick={use}>Place signature</button>
    </div>
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
    width: min(620px, calc(100vw - 32px));
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  .muted {
    color: var(--muted);
    font-size: 12px;
  }
  .saved {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .saved-item {
    position: relative;
  }
  .saved-pick {
    height: 44px;
    padding: 4px 8px;
    background: #fff;
  }
  .saved-pick img {
    height: 34px;
    display: block;
  }
  .x {
    position: absolute;
    top: -6px;
    right: -6px;
    width: 18px;
    height: 18px;
    min-height: 0;
    padding: 0;
    border-radius: 50%;
    font-size: 12px;
    line-height: 1;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .tabs [role="tab"] {
    border-color: transparent;
    background: transparent;
  }
  .tabs [role="tab"].on {
    background: var(--btn-hover);
  }
  .spacer {
    flex: 1;
  }
  .ink {
    width: 22px;
    height: 22px;
    min-height: 0;
    padding: 0;
    border-radius: 50%;
    border: 2px solid transparent;
  }
  .ink.on {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--bg) inset;
  }
  .area {
    position: relative;
    min-height: 200px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: #fff;
    color: #111;
    overflow: hidden;
  }
  .pad {
    display: block;
    width: 100%;
    height: 200px;
    touch-action: none;
    cursor: crosshair;
  }
  .clear {
    position: absolute;
    top: 6px;
    right: 8px;
    border: none;
    background: none;
    color: #555;
    text-decoration: underline;
  }
  .line {
    position: absolute;
    left: 24px;
    right: 24px;
    bottom: 48px;
    border-bottom: 1px dashed #bbb;
    pointer-events: none;
  }
  .typed-input {
    margin: 12px;
    width: calc(100% - 24px);
    box-sizing: border-box;
    background: #fff;
    color: #111;
  }
  .fonts {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
    padding: 0 12px 12px;
  }
  .font {
    background: #fff;
    border: 1px solid #ddd;
    font-size: 26px;
    min-height: 54px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .font.on {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .image-pick {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 16px;
    color: #111;
  }
  .image-pick img {
    max-width: 100%;
    max-height: 120px;
  }
  .image-pick button {
    background: #f4f4f6;
    color: #111;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .remember {
    font-size: 12px;
    color: var(--muted);
  }
  .bad {
    color: var(--danger);
    margin: 0;
  }
  .link {
    text-decoration: underline;
  }
</style>
