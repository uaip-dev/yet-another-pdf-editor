// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import process from "node:process";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      fallback: "index.html",
    }),
    alias: {
      // Extension UI slot (docs/EXTENSIONS.md). Community builds use the empty
      // components in src/lib/pro; other builds set YAPE_PRO_UI to their own folder.
      $pro: process.env.YAPE_PRO_UI || "src/lib/pro",
    },
  },
};

export default config;
