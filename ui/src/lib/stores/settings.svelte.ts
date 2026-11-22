/**
 * Settings shared by every window. Rust owns the values; this store mirrors
 * them and applies the theme to the document.
 */
import { commands, events, type Settings, type Theme } from "$lib/ipc";

const prefersDark = window.matchMedia("(prefers-color-scheme: dark)");

class SettingsStore {
  current = $state<Settings>({
    theme: "system",
    agents: [],
    active_agent: null,
    permissions: { network: false, tools: false },
    providers: [],
    default_provider: null,
    local: { device: "auto" },
    hf_token_set: false,
  });
  /** The theme actually shown, with "system" resolved. */
  resolvedTheme = $state<"dark" | "light">(prefersDark.matches ? "dark" : "light");

  async init() {
    this.apply(await commands.getSettings());
    await events.onSettingsChanged((s) => this.apply(s));
    prefersDark.addEventListener("change", () => this.apply(this.current));
  }

  setTheme(theme: Theme) {
    return commands.setTheme(theme);
  }

  private apply(s: Settings) {
    this.current = s;
    this.resolvedTheme = s.theme === "system" ? (prefersDark.matches ? "dark" : "light") : s.theme;
    document.documentElement.dataset.theme = this.resolvedTheme;
  }
}

export const settings = new SettingsStore();
