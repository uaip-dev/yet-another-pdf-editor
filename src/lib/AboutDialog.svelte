<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import Icon from "./Icon.svelte";

  interface Props {
    /** Looks for a newer release; returns its version, or null when up to date. */
    oncheck: () => Promise<string | null>;
    /** Installs the update found by `oncheck` and restarts. */
    oninstall: () => void;
    onclose: () => void;
    /** "Community" or the name of the edition provided by extensions. */
    edition?: string;
  }

  let { oncheck, oninstall, onclose, edition = "Community" }: Props = $props();

  const WEBSITE = "https://yetanotherpdf.dev";
  const REPO = "https://github.com/uaip-dev/yet-another-pdf-editor";
  const EMAIL = "info@uaip.dev";

  let version = $state("");
  let checking = $state(false);
  let status: "idle" | "latest" | "available" | "error" = $state("idle");
  let available = $state("");

  onMount(async () => {
    try {
      version = await getVersion();
    } catch {
      version = "";
    }
  });

  async function check() {
    checking = true;
    try {
      const v = await oncheck();
      status = v ? "available" : "latest";
      available = v ?? "";
    } catch {
      status = "error";
    } finally {
      checking = false;
    }
  }

  const open = (url: string) => openUrl(url).catch(() => {});

  const links: { label: string; detail: string; url: string; icon: import("./Icon.svelte").IconName }[] = [
    { label: "Website", detail: `${WEBSITE}/`, url: WEBSITE, icon: "globe" },
    { label: "Source code", detail: REPO, url: REPO, icon: "file" },
    { label: "Release notes", detail: "What's new in each version", url: `${REPO}/releases`, icon: "note" },
    { label: "Report an issue", detail: "Bugs and feature requests", url: `${REPO}/issues`, icon: "comment" },
  ];
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="about-title">
    <button class="close" onclick={onclose} aria-label="Close"><Icon name="close" size={18} /></button>

    <div class="head">
      <img src="/app-icon.png" alt="" width="72" height="72" />
      <div>
        <h2 id="about-title">Yet Another PDF Editor</h2>
        <p class="version">Version {version || "—"} · {edition} edition</p>
        <p class="tagline">A lightweight, open-source PDF editor for Windows, macOS and Linux.</p>
      </div>
    </div>

    <div class="update">
      {#if status === "available"}
        <span>Version {available} is available.</span>
        <button class="primary" onclick={oninstall}>Install and restart</button>
      {:else}
        <span class="muted">
          {#if status === "latest"}You're using the latest version.
          {:else if status === "error"}Couldn't check for updates. Check your internet connection.
          {:else}Updates are checked automatically when the app starts.{/if}
        </span>
        <button onclick={check} disabled={checking}>{checking ? "Checking…" : "Check for updates"}</button>
      {/if}
    </div>

    <ul class="links">
      {#each links as l}
        <li>
          <button class="link-row" onclick={() => open(l.url)}>
            <span class="link-icon"><Icon name={l.icon} size={16} /></span>
            <span class="link-text"><strong>{l.label}</strong><span class="muted">{l.detail}</span></span>
            <span class="ext" aria-hidden="true">↗</span>
          </button>
        </li>
      {/each}
    </ul>

    <section class="contact">
      <h3>Contact &amp; sponsorship</h3>
      <p class="muted">
        Questions, feedback, partnerships or sponsoring the project — we'd love to hear from you.
        Sponsorship helps fund development, code signing and new features.
      </p>
      <button class="mail" onclick={() => open(`mailto:${EMAIL}?subject=${encodeURIComponent("Yet Another PDF Editor")}`)}>
        <Icon name="mail" size={16} />{EMAIL}
      </button>
    </section>

    <footer>
      <p>
        Free and open source under the
        <button class="inline" onclick={() => open(`${REPO}/blob/main/LICENSE`)}>MIT license</button>.
        © 2026 Yet Another PDF Editor contributors.
      </p>
      <p class="muted">
        Built with Rust, Tauri, Svelte and PDFium. Your documents stay on your computer; the app
        only contacts GitHub to check for updates.
      </p>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.4);
    display: grid;
    place-items: center;
    z-index: 20;
    padding: 16px;
  }
  .dialog {
    position: relative;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 14px;
    width: min(520px, 100%);
    max-height: calc(100vh - 32px);
    overflow: auto;
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    box-shadow: 0 20px 60px rgb(0 0 0 / 0.3);
  }
  .close {
    position: absolute;
    top: 12px;
    right: 12px;
    width: 32px;
    min-height: 32px;
    padding: 0;
    display: grid;
    place-items: center;
    border-color: transparent;
    background: transparent;
  }
  .head {
    display: flex;
    gap: 16px;
    align-items: center;
  }
  .head img {
    flex: none;
    border-radius: 16px;
    filter: drop-shadow(0 6px 14px rgb(90 60 220 / 0.25));
  }
  h2 {
    margin: 0;
    font-size: 20px;
  }
  .version {
    margin: 2px 0 6px;
    font-size: 13px;
    color: var(--accent);
    font-weight: 600;
  }
  .tagline {
    margin: 0;
    color: var(--muted);
    font-size: 14px;
  }
  .update {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-2);
    font-size: 13px;
  }
  .update button {
    flex: none;
  }
  .links {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
  }
  .links li + li {
    border-top: 1px solid var(--border);
  }
  .link-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border: none;
    border-radius: 0;
    background: transparent;
    text-align: left;
  }
  .link-row:hover {
    background: var(--btn-hover);
  }
  .link-icon {
    width: 30px;
    height: 30px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 8px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .link-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    font-size: 13px;
  }
  .link-text strong {
    font-size: 14px;
    font-weight: 600;
  }
  .link-text .muted {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ext {
    color: var(--muted);
  }
  .contact h3 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .contact p {
    margin: 0 0 10px;
    font-size: 13px;
  }
  .mail {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .mail:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
  footer {
    border-top: 1px solid var(--border);
    padding-top: 12px;
    font-size: 12px;
  }
  footer p {
    margin: 0 0 4px;
  }
  .inline {
    border: none;
    background: none;
    padding: 0;
    min-height: 0;
    color: var(--accent);
    text-decoration: underline;
    font-size: inherit;
  }
  .muted {
    color: var(--muted);
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
</style>
