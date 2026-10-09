<script lang="ts">
  // Extension slot: extra items in the toolbar's More menu. Empty in the
  // Community edition. See docs/EXTENSIONS.md.
  import type { ExtensionContext } from "$lib/extension";

  let {}: { ctx: ExtensionContext; close: () => void } = $props();
</script>
