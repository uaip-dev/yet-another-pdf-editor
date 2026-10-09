<script lang="ts" module>
  // 24x24 stroke icons.
  const PATHS = {
    sidebar: "M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM9 3v18",
    open: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
    print: "M6 9V3h12v6M6 18H4a1 1 0 0 1-1-1v-6a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v6a1 1 0 0 1-1 1h-2M6 14h12v7H6z",
    search: "M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zM21 21l-5-5",
    prev: "M15 18l-6-6 6-6",
    next: "M9 18l6-6-6-6",
    up: "M18 15l-6-6-6 6",
    down: "M6 9l6 6 6-6",
    plus: "M12 5v14M5 12h14",
    minus: "M5 12h14",
    close: "M6 6l12 12M18 6L6 18",
    file: "M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6",
    edit: "M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16zM13.5 6.5l4 4",
    undo: "M9 14L4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3",
    redo: "M15 14l5-5-5-5M20 9H10a6 6 0 0 0 0 12h3",
    save: "M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM7 3v5h8M7 21v-7h10v7",
    text: "M4 7V5h16v2M12 5v14M9 19h6",
    image: "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM8.5 10a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM21 15l-5-5L5 21",
    trash: "M3 6h18M8 6V4h8v2M6 6l1 14a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-14M10 11v6M14 11v6",
    replace: "M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M3 21v-5h5",
    pointer: "M4 4l7 17 2.5-7.5L21 11z",
    comment: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z",
    highlight: "M9 11l-6 6v3h9l3-3M22 12l-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4",
    underline: "M6 4v6a6 6 0 0 0 12 0V4M4 20h16",
    strike: "M16 4H9a3 3 0 0 0-2.83 4M14 12a4 4 0 0 1 0 8H6M4 12h16",
    note: "M15.5 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V8.5zM15 3v6h6M8 13h8M8 17h5",
    pen: "M3 17c3-3 5-1 7 1s4 3 7-1M14.5 4.5l5 5L12 17H7v-5z",
    square: "M4 4h16v16H4z",
    circle: "M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18z",
    signature: "M3 17c2.5 0 4-6 6-6s1 6 3 6 2-3 3.5-3 1.5 3 3 3h2.5M3 21h18",
    rotateLeft: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5",
    rotateRight: "M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5",
    insert: "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM12 8v8M8 12h8",
    extract: "M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h6M14 3l6 6M14 3v6h6M16 15h6M19 12l3 3-3 3",
    more: "M5 12h.01M12 12h.01M19 12h.01",
    redact: "M3 5h18v6H3zM3 15h8M3 19h12M15 15l6 6M21 15l-6 6",
    lock: "M6 11h12v10H6zM8 11V7a4 4 0 0 1 8 0v4",
    info: "M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM12 11v6M12 7.5h.01",
    shrink: "M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7",
    export: "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM8.5 10a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM21 15l-5-5L5 21",
    globe: "M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3z",
    mail: "M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zM3 7l9 6 9-6",
    scissors: "M6 3a3 3 0 1 1 0 6 3 3 0 0 1 0-6zM6 15a3 3 0 1 1 0 6 3 3 0 0 1 0-6zM20 4L8.1 15.9M14.5 14.5L20 20M8.1 8.1L12 12",
  } as const;
  export type IconName = keyof typeof PATHS;
</script>

<script lang="ts">
  let { name, size = 18 }: { name: IconName; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"><path d={PATHS[name]} /></svg
>

<style>
  svg {
    display: block;
    flex: none;
  }
</style>
