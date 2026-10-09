<script lang="ts">
  import { fillFields, type DocState, type FieldInfo, type FieldValue, type Rect4 } from "./api";

  interface Props {
    docId: number;
    page: number;
    /** Page size in points. */
    width: number;
    height: number;
    /** CSS px per point. */
    scale: number;
    revision: number;
    fields: FieldInfo[];
    run: (op: () => Promise<DocState>) => Promise<boolean>;
  }

  let { docId, page, width, height, scale, revision, fields, run }: Props = $props();

  // Text being typed, per field id (committed on blur / Enter).
  let drafts: Record<number, string> = $state({});

  const pos = (r: Rect4) =>
    `left:${(r[0] / width) * 100}%;top:${(r[1] / height) * 100}%;` +
    `width:${((r[2] - r[0]) / width) * 100}%;height:${((r[3] - r[1]) / height) * 100}%`;

  const fontPx = (f: FieldInfo) => Math.min((f.rect[3] - f.rect[1]) * 0.62, 12) * scale;

  function set(f: FieldInfo, value: FieldValue) {
    run(() => fillFields(docId, page, revision, [[f.id, value]]));
  }

  function commitText(f: FieldInfo) {
    const v = drafts[f.id];
    delete drafts[f.id];
    if (v !== undefined && v !== f.value) set(f, { type: "text", value: v });
  }

  function onTextKey(e: KeyboardEvent, f: FieldInfo) {
    e.stopPropagation();
    if (e.key === "Enter" && !f.multiline) (e.currentTarget as HTMLElement).blur();
    if (e.key === "Escape") {
      delete drafts[f.id];
      (e.currentTarget as HTMLElement).blur();
    }
  }
</script>

<div class="forms">
  {#each fields as f (f.id)}
    {#if !f.readOnly}
      {#if f.kind === "text"}
        {#if f.multiline}
          <textarea
            class="field text"
            style="{pos(f.rect)};font-size:{fontPx(f)}px"
            value={drafts[f.id] ?? f.value}
            oninput={(e) => (drafts[f.id] = e.currentTarget.value)}
            onblur={() => commitText(f)}
            onkeydown={(e) => onTextKey(e, f)}
            onpointerdown={(e) => e.stopPropagation()}
            title={f.name}
            aria-label={f.name}
          ></textarea>
        {:else}
          <input
            class="field text"
            type={f.password ? "password" : "text"}
            style="{pos(f.rect)};font-size:{fontPx(f)}px"
            value={drafts[f.id] ?? f.value}
            oninput={(e) => (drafts[f.id] = e.currentTarget.value)}
            onblur={() => commitText(f)}
            onkeydown={(e) => onTextKey(e, f)}
            onpointerdown={(e) => e.stopPropagation()}
            title={f.name}
            aria-label={f.name}
          />
        {/if}
      {:else if f.kind === "checkbox" || f.kind === "radio"}
        <button
          class="field toggle"
          class:round={f.kind === "radio"}
          style={pos(f.rect)}
          role={f.kind === "radio" ? "radio" : "checkbox"}
          aria-checked={f.checked}
          aria-label={f.name}
          title={f.name}
          onpointerdown={(e) => e.stopPropagation()}
          onclick={() => {
            if (f.kind === "radio" && f.checked) return;
            set(f, { type: "checked", value: !f.checked });
          }}
        ></button>
      {:else if (f.kind === "combo" || f.kind === "list") && f.options.length}
        <select
          class="field choice"
          style="{pos(f.rect)};font-size:{fontPx(f)}px"
          value={f.selected ?? -1}
          onchange={(e) => set(f, { type: "select", value: Number(e.currentTarget.value) })}
          onpointerdown={(e) => e.stopPropagation()}
          title={f.name}
          aria-label={f.name}
        >
          {#if f.selected === null}<option value={-1} disabled hidden></option>{/if}
          {#each f.options as label, i}<option value={i}>{label}</option>{/each}
        </select>
      {/if}
    {/if}
  {/each}
</div>

<style>
  .forms {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .field {
    position: absolute;
    box-sizing: border-box;
    margin: 0;
    padding: 0 2px;
    min-height: 0;
    border: 1px solid transparent;
    border-radius: 2px;
    background: rgb(80 130 255 / 0.12);
    pointer-events: auto;
    font-family: Helvetica, Arial, sans-serif;
    /* The PDF renders the value; the overlay text shows only while editing. */
    color: transparent;
  }
  .field:hover {
    border-color: rgb(80 130 255 / 0.7);
  }
  .field.text:focus,
  .field.choice:focus {
    color: #000;
    background: #fff;
    border-color: var(--accent);
    outline: none;
  }
  textarea.field {
    resize: none;
    line-height: 1.15;
  }
  .toggle {
    cursor: pointer;
  }
  .toggle.round {
    border-radius: 50%;
  }
  .choice {
    cursor: pointer;
  }
  .choice option {
    color: #000;
  }
</style>
