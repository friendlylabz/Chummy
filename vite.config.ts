import { resolve } from "node:path";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const ui = resolve(import.meta.dirname, "ui");

// Every window is its own page, so each loads only the code it needs.
// Add a window here, in src-tauri/src/windows.rs and in src-tauri/capabilities/.
const pages = {
  main: resolve(ui, "index.html"),
  editor: resolve(ui, "editor.html"),
  settings: resolve(ui, "settings.html"),
  scheduler: resolve(ui, "scheduler.html"),
};

export default defineConfig({
  root: ui,
  plugins: [svelte()],
  resolve: {
    alias: { $lib: resolve(ui, "src/lib"), $theme: resolve(ui, "src/theme") },
  },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_"],
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    rollupOptions: { input: pages },
  },
});
