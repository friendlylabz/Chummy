/**
 * Shared entry point for every window: loads base styles, syncs the theme,
 * then mounts the window's root component.
 */
import "$theme/base.css";
import { mount, type Component } from "svelte";
import { settings } from "$lib/stores/settings.svelte";

export async function boot(root: Component) {
  // Apply the theme before mounting so there is no flash of the wrong colors.
  await settings.init().catch((e) => console.error("[chummy] settings init failed", e));
  return mount(root, { target: document.getElementById("app")! });
}
