<script lang="ts">
  // Extension slot: always mounted; extensions render their dialogs and
  // panels here. Empty in the Community edition. See docs/EXTENSIONS.md.
  import type { ExtensionContext } from "$lib/extension";

  let {}: { ctx: ExtensionContext } = $props();
</script>
