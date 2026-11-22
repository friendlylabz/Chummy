<script lang="ts" module>
  // Simple 24×24 stroke icons. Add new ones here; keep them geometric and quiet.
  const paths = {
    menu: "M4 7h16M4 12h16M4 17h10",
    "panel-right": "M4 5h16v14H4zM15 5v14",
    chat: "M5 6.5A2.5 2.5 0 0 1 7.5 4h9A2.5 2.5 0 0 1 19 6.5v7a2.5 2.5 0 0 1-2.5 2.5H11l-4 3.5V16h0a2.5 2.5 0 0 1-2-2.5z",
    flow: "M4 5h5v5H4zM15 14h5v5h-5zM9 7.5h3a2 2 0 0 1 2 2v5a2 2 0 0 0 2 2",
    sliders: "M5 7h9M18 7h1M5 17h3M12 17h7M14 5v4M10 15v4",
    clock: "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 7.5V12l3 2",
    plus: "M12 5v14M5 12h14",
    import: "M12 4v10M8 10l4 4 4-4M5 16v3h14v-3",
    sun: "M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7zM12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6 7 7M17 17l1.4 1.4M5.6 18.4 7 17M17 7l1.4-1.4",
    moon: "M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z",
    monitor: "M4 5h16v11H4zM9 20h6M12 16v4",
    send: "M12 19V6M6.5 11.5 12 6l5.5 5.5",
    check: "M5 12.5l4.5 4.5L19 7.5",
    bot: "M7 9h10a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2zM12 5v4M9.5 14h.01M14.5 14h.01",
    folder: "M4 7a2 2 0 0 1 2-2h3.5l2 2H18a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z",
    shield: "M12 3.5 5 6v5.5c0 4.2 3 7.6 7 9 4-1.4 7-4.8 7-9V6z",
    info: "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 11v5M12 8h.01",
    close: "M6 6l12 12M18 6 6 18",
    history: "M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3h3M12 8v4l2.5 1.5",
    play: "M8 5.5v13l10-6.5z",
    palette: "M12 3.5a8.5 8.5 0 0 0 0 17c1.1 0 1.6-.8 1.6-1.6 0-1.3-1-1.6-1-2.6 0-.9.7-1.6 1.6-1.6h2.3A4 4 0 0 0 20.5 10c0-3.6-3.8-6.5-8.5-6.5zM8 11h.01M10.5 7.5h.01M15 8h.01",
    alert: "M12 4 2.8 19.5h18.4zM12 10v4M12 17h.01",
    "x-circle": "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM9.5 9.5l5 5M14.5 9.5l-5 5",
    "check-circle": "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM8.5 12.2l2.3 2.3 4.7-4.8",
    minus: "M7 12h10",
    download: "M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19h14",
    export: "M12 15V4M7.5 8.5 12 4l4.5 4.5M5 16v3h14v-3",
    trash: "M5 7h14M10 7V5h4v2M7 7l1 12h8l1-12M10.5 11v5M13.5 11v5",
    key: "M14.5 4a5.5 5.5 0 0 0-5.2 7.3L4 16.6V20h3.4v-2h2v-2h2l1.3-1.3A5.5 5.5 0 1 0 14.5 4zM16 8h.01",
    cpu: "M7 7h10v10H7zM10 10h4v4h-4zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4",
    stop: "M7 7h10v10H7z",
    refresh: "M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.5H16",
    globe: "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM3.5 12h17M12 3.5c2.3 2.4 3.4 5.2 3.4 8.5s-1.1 6.1-3.4 8.5c-2.3-2.4-3.4-5.2-3.4-8.5s1.1-6.1 3.4-8.5z",
    search: "M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13zM15.5 15.5 20 20",
    "chevron-left": "M14.5 6 8.5 12l6 6",
    "chevron-down": "M6 9.5l6 6 6-6",
    external: "M14 4h6v6M20 4l-9 9M18 14v5H5V6h5",
    cloud: "M7 18a4.5 4.5 0 0 1-.5-9 6 6 0 0 1 11.5 1.5A3.75 3.75 0 0 1 17.5 18z",
    laptop: "M5 6h14v9H5zM3 18h18",
    edit: "M15 5l4 4L9 19H5v-4z",
    star: "M12 4l2.4 5 5.4.6-4 3.7 1.1 5.4L12 16l-4.9 2.7 1.1-5.4-4-3.7 5.4-.6z",
  } as const;
  export type IconName = keyof typeof paths;
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
  stroke-width="1.7"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={paths[name]} />
</svg>
