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

// ---------------------------------------------------------------- types

export type Theme = "system" | "dark" | "light";
export type AppWindow = "editor" | "settings" | "scheduler";
export type ProviderKind =
  | "open_ai_compatible"
  | "open_ai"
  | "ollama"
  | "anthropic"
  | "gemini"
  | "local";
export type Precision = "half" | "int8" | "int4";

export interface LocalModel {
  repo: string;
  revision: string;
  dir_name: string;
  parser: string;
  precision: Precision;
}

export interface ProviderConfig {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  model: string;
  local?: LocalModel | null;
  temperature?: number | null;
}

export interface ProviderView extends ProviderConfig {
  has_key: boolean;
}

export interface Workspace {
  path: string;
  confirmed_at: number;
}

export interface Settings {
  theme: Theme;
  workspace: Workspace | null;
  permissions: { network: boolean; tools: boolean };
  providers: ProviderView[];
  default_provider: string | null;
  local: { device: string };
  hf_token_set: boolean;
}

export interface SetupStatus {
  workspace_configured: boolean;
  model_configured: boolean;
  loop_configured: boolean;
}

export interface AppInfo {
  version: string;
  data_dir: string;
  recovery: { kind: "used_backup" | "used_defaults"; reason: string } | null;
}

export interface Preset {
  id: string;
  label: string;
  kind: ProviderKind;
  base_url: string;
  needs_key: boolean;
  local: boolean;
  hint: string;
}

export type StepStatus = "pass" | "warn" | "fail" | "skipped";
export interface ProbeStep {
  name: string;
  status: StepStatus;
  detail: string;
  ms: number;
}
export interface ProbeReport {
  steps: ProbeStep[];
  models: string[];
  ok: boolean;
}
export interface TestOutcome {
  needs_runtime: boolean;
  report: ProbeReport | null;
}

export interface Gpu {
  name: string;
  vram_bytes: number;
}
export interface Hardware {
  os: string;
  arch: string;
  ram_bytes: number;
  nvidia: Gpu[];
  apple_silicon: boolean;
}

export interface Cell {
  precision: Precision;
  context: number;
  bytes: number;
}
export interface Fit {
  label: string;
  kind: "nvidia_gpu" | "apple_silicon" | "cpu_only";
  usable_bytes: number;
  best: Cell | null;
  this_computer: boolean;
}
export interface Estimates {
  shape: { params: number; layers: number; kv_heads: number; head_dim: number; max_context: number };
  table: Cell[];
  fits: Fit[];
}
export interface Check {
  label: string;
  status: "pass" | "warn" | "fail";
  detail: string;
}
export interface Inspection {
  repo: string;
  revision: string;
  architecture: string | null;
  parser: string | null;
  parser_label: string | null;
  params: number | null;
  context_length: number | null;
  files: { path: string; size: number }[];
  download_bytes: number;
  checks: Check[];
  ok: boolean;
  estimates: Estimates | null;
  recommended_precision: Precision;
  hardware: Hardware;
}

export interface RuntimeInfo {
  python: string;
  torch: string;
  transformers: string;
  device: string;
  gpu: string | null;
  cuda: string | null;
  installed_at: number;
}

export type EngineStatus =
  | { state: "stopped" }
  | { state: "loading"; model: string }
  | { state: "ready"; model: string; device: string }
  | { state: "failed"; error: string };

export interface InstalledModel {
  dir_name: string;
  manifest: { repo: string; revision: string; parser: string; files: string[]; bytes: number; downloaded_at: number };
}

export interface LocalStatus {
  runtime: RuntimeInfo | null;
  runtime_installing: boolean;
  runtime_download_gb: number;
  hardware: Hardware;
  hardware_summary: string;
  engine: EngineStatus;
  models: InstalledModel[];
  downloading: string[];
  hf_token_set: boolean;
  device: string;
}

export interface Progress {
  repo: string;
  file: string;
  file_index: number;
  file_count: number;
  done_bytes: number;
  total_bytes: number;
  bytes_per_sec: number;
}

export type DownloadEvent =
  | { kind: "progress"; progress: Progress }
  | { kind: "done"; repo: string; provider_id: string }
  | { kind: "failed"; repo: string; error: string }
  | { kind: "cancelled"; repo: string };

export interface RuntimeStep {
  index: number;
  count: number;
  phase: string;
  line: string | null;
}
export type RuntimeEvent =
  | { kind: "step"; step: RuntimeStep }
  | { kind: "done"; info: RuntimeInfo }
  | { kind: "failed"; error: string }
  | { kind: "cancelled" };

// ---------------------------------------------------------------- browser fallback

const inTauri = isTauri();

const mock = {
  settings: {
    theme: "system",
    workspace: null,
    permissions: { network: false, tools: false },
    providers: [],
    default_provider: null,
    local: { device: "auto" },
    hf_token_set: false,
  } as Settings,
  listeners: new Set<(s: Settings) => void>(),
};

function call<T>(cmd: string, args?: Record<string, unknown>, fallback?: () => T): Promise<T> {
  if (inTauri) return invoke<T>(cmd, args);
  if (fallback) return Promise.resolve(fallback());
  return Promise.reject(new Error(`${cmd} needs the desktop app`));
}

/** Turns whatever a command threw into a readable message. */
export function errorText(e: unknown): string {
  const s = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  return s.charAt(0).toUpperCase() + s.slice(1);
}

// ---------------------------------------------------------------- commands

export const commands = {
  setupStatus: () =>
    call<SetupStatus>("setup_status", undefined, () => ({
      workspace_configured: false,
      model_configured: false,
      loop_configured: false,
    })),
  openWindow: (window: AppWindow) => call<void>("open_window", { window }, () => undefined),
  getSettings: () => call<Settings>("get_settings", undefined, () => mock.settings),
  setTheme: (theme: Theme) =>
    call<void>("set_theme", { theme }, () => {
      mock.settings = { ...mock.settings, theme };
      mock.listeners.forEach((cb) => cb(mock.settings));
    }),
  appInfo: () =>
    call<AppInfo>("app_info", undefined, () => ({ version: "dev", data_dir: "(browser preview)", recovery: null })),
  revealDataDir: () => call<void>("reveal_data_dir"),

  chooseWorkspace: () => call<boolean>("choose_workspace", undefined, () => false),
  clearWorkspace: () => call<void>("clear_workspace"),
  setPermission: (kind: "network" | "tools", enabled: boolean) =>
    call<boolean>("set_permission", { kind, enabled }, () => false),

  listPresets: () => call<Preset[]>("list_presets", undefined, () => []),
  saveProvider: (config: ProviderConfig, apiKey: string | null) =>
    call<string>("save_provider", { config, apiKey }),
  deleteProvider: (id: string) => call<void>("delete_provider", { id }),
  setDefaultProvider: (id: string) => call<void>("set_default_provider", { id }),
  listModels: (config: ProviderConfig, apiKey: string | null) =>
    call<string[]>("list_models", { config, apiKey }),
  testProvider: (config: ProviderConfig, apiKey: string | null) =>
    call<TestOutcome>("test_provider", { config, apiKey }),

  localStatus: () => call<LocalStatus>("local_status"),
  inspectModel: (repo: string) => call<Inspection>("inspect_model", { repo }),
  downloadModel: (repo: string, precision: Precision) => call<void>("download_model", { repo, precision }),
  cancelDownload: (repo: string) => call<void>("cancel_download", { repo }),
  deleteLocalModel: (dirName: string) => call<boolean>("delete_local_model", { dirName }),
  installRuntime: () => call<boolean>("install_runtime"),
  cancelRuntimeInstall: () => call<void>("cancel_runtime_install"),
  removeRuntime: () => call<boolean>("remove_runtime"),
  setHfToken: (token: string | null) => call<void>("set_hf_token", { token }),
  setLocalDevice: (device: string) => call<void>("set_local_device", { device }),
  stopEngine: () => call<void>("stop_engine"),
  engineLog: () => call<string[]>("engine_log", undefined, () => []),
};

// ---------------------------------------------------------------- events

function on<T>(name: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (inTauri) return listen<T>(name, (e) => cb(e.payload));
  return Promise.resolve(() => {});
}

export const events = {
  onSettingsChanged(cb: (s: Settings) => void): Promise<UnlistenFn> {
    if (inTauri) return on("settings:changed", cb);
    mock.listeners.add(cb);
    return Promise.resolve(() => mock.listeners.delete(cb));
  },
  onSetupChanged: (cb: (s: SetupStatus) => void) => on("setup:changed", cb),
  onDownload: (cb: (e: DownloadEvent) => void) => on("local:download", cb),
  onRuntime: (cb: (e: RuntimeEvent) => void) => on("local:runtime", cb),
  onEngine: (cb: (e: EngineStatus) => void) => on("local:engine", cb),
};
