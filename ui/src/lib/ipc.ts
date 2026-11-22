/**
 * Typed bridge to the Rust backend. Every command and event the UI uses goes
 * through this file, so the IPC surface stays in one place.
 *
 * Keep in sync with src-tauri/src/commands/ and src-tauri/src/events.rs.
 *
 * Outside Tauri (plain `npm run dev` in a browser) calls fall back to
 * first-launch defaults, so the UI can be worked on without the Rust side.
 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Theme = "system" | "dark" | "light";
export type AppWindow = "editor" | "settings" | "scheduler";

export interface Settings {
  theme: Theme;
}

export interface SetupStatus {
  model_configured: boolean;
  loop_configured: boolean;
}

const inTauri = isTauri();

// Browser-only fallback state.
const mock = {
  settings: { theme: "system" } as Settings,
  setup: { model_configured: false, loop_configured: false } as SetupStatus,
  listeners: new Set<(s: Settings) => void>(),
};

export const commands = {
  setupStatus(): Promise<SetupStatus> {
    return inTauri ? invoke("setup_status") : Promise.resolve(mock.setup);
  },

  openWindow(window: AppWindow): Promise<void> {
    if (inTauri) return invoke("open_window", { window });
    console.info(`[chummy] open_window(${window}) needs the desktop app`);
    return Promise.resolve();
  },

  getSettings(): Promise<Settings> {
    return inTauri ? invoke("get_settings") : Promise.resolve(mock.settings);
  },

  setTheme(theme: Theme): Promise<void> {
    if (inTauri) return invoke("set_theme", { theme });
    mock.settings = { ...mock.settings, theme };
    mock.listeners.forEach((cb) => cb(mock.settings));
    return Promise.resolve();
  },
};

export const events = {
  onSettingsChanged(cb: (s: Settings) => void): Promise<UnlistenFn> {
    if (inTauri) return listen<Settings>("settings:changed", (e) => cb(e.payload));
    mock.listeners.add(cb);
    return Promise.resolve(() => mock.listeners.delete(cb));
  },

  onSetupChanged(cb: (s: SetupStatus) => void): Promise<UnlistenFn> {
    if (inTauri) return listen<SetupStatus>("setup:changed", (e) => cb(e.payload));
    return Promise.resolve(() => {});
  },
};
