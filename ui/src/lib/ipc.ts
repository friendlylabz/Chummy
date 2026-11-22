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

export interface Agent {
  id: string;
  name: string;
  loop_id: string;
  model: string | null;
  workspace: Workspace;
  created_at: number;
}

export interface Settings {
  theme: Theme;
  agents: Agent[];
  active_agent: string | null;
  permissions: { network: boolean; tools: boolean; confirm_online_tools: boolean };
  providers: ProviderView[];
  default_provider: string | null;
  local: { device: string };
  hf_token_set: boolean;
  tools: Record<string, { enabled: boolean; network: boolean; ask: boolean }>;
  general: General;
}

export type CloseAction = "ask" | "quit" | "tray";
export interface General {
  close: CloseAction;
  notifications: boolean;
  max_job_runs: number;
}

/** What's going on, when the user closes or quits. */
export interface Activity {
  runs: number;
  waiting: number;
  scheduled: number;
  paused: boolean;
  tray: boolean;
  reason: "close" | "quit";
}

export interface SetupStatus {
  model_configured: boolean;
  loop_configured: boolean;
  agent_configured: boolean;
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

// ---- loops

export type ValueKind = "text" | "list" | "exchange" | "any";
export interface PortSpec {
  name: string;
  label: string;
  kind: ValueKind;
}
export interface Position {
  x: number;
  y: number;
}
export interface Block {
  id: string;
  type: string;
  label: string;
  position: Position;
  params: Record<string, unknown>;
}
export interface PortRef {
  block: string;
  port: string;
}
export interface Link {
  from: PortRef;
  to: PortRef;
}
export interface RunLimits {
  max_steps: number;
  max_llm_calls: number;
  max_tool_calls?: number;
}
export interface FunctionInput {
  name: string;
  label: string;
  to: PortRef;
}
export interface FunctionOutput {
  name: string;
  label: string;
  from: PortRef;
}
export interface FunctionDef {
  id: string;
  name: string;
  description: string;
  inputs: FunctionInput[];
  outputs: FunctionOutput[];
  blocks: Block[];
  links: Link[];
}
export interface LoopFile {
  format: "chummy.loop";
  version: number;
  name: string;
  description: string;
  limits: RunLimits;
  blocks: Block[];
  links: Link[];
  functions?: FunctionDef[];
}
export interface Issue {
  severity: "error" | "warning";
  block: string | null;
  /** The function the block is in, if any. */
  function?: string;
  message: string;
}
export interface BlockPorts {
  inputs: PortSpec[];
  outputs: PortSpec[];
}
export interface Analysis {
  ports: Record<string, BlockPorts>;
  functions: Record<string, { ports: BlockPorts; blocks: Record<string, BlockPorts> }>;
  issues: Issue[];
  ok: boolean;
}
export type ParamKind =
  | { kind: "text" }
  | { kind: "multiline" }
  | { kind: "number"; min: number; max: number; step: number }
  | { kind: "integer"; min: number; max: number }
  | { kind: "bool" }
  | { kind: "model" }
  | { kind: "choice"; options: { value: string; label: string }[] }
  | { kind: "roles" }
  | { kind: "tools" };
export type ParamSpec = ParamKind & { key: string; label: string; help: string; optional: boolean };
export type Category = "start" | "think" | "you" | "exchange" | "text" | "flow" | "workspace" | "finish";
export interface BlockInfo {
  kind: string;
  label: string;
  category: Category;
  description: string;
  params: ParamSpec[];
  default_params: Record<string, unknown>;
  palette: boolean;
}
export interface Template {
  id: string;
  name: string;
  description: string;
  file: LoopFile;
}
export interface LoopSummary {
  id: string;
  name: string;
  description: string;
  blocks: number;
  ok: boolean;
  modified: number;
}

// ---- chat and runs

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "notice" | "question" | "answer";
  text: string;
  at: number;
  /** A job's name, for questions that came from a job. */
  from?: string;
  run?: { status: string; steps: number; llm_calls: number; tool_calls?: number };
}
export interface RunInfo {
  run_id: string;
  agent_id: string | null;
  title: string;
  source: "chat" | "test" | "job";
  job_id?: string;
  started_at: number;
}
export type BlockState = "waiting" | "running" | "done" | "failed";
export type FlowEvent =
  | { type: "block_state"; block: string; label: string; state: BlockState }
  | { type: "block_output"; block: string; port: string; preview: string }
  | { type: "llm_text"; block: string; text: string }
  | { type: "tool_call"; block: string; tool: string; arguments: string }
  | { type: "tool_result"; block: string; tool: string; preview: string; is_error: boolean }
  | { type: "notice"; block: string; text: string }
  | { type: "reply"; block: string; text: string }
  | { type: "asked"; block: string; text: string }
  | { type: "answered"; block: string };
export type RunStatus =
  | { status: "completed" }
  | { status: "stopped" }
  | { status: "limit_reached"; message: string }
  | { status: "failed"; block: string | null; error: string };
export interface RunOutcome {
  status: RunStatus;
  replies: string[];
  steps: number;
  llm_calls: number;
  tool_calls: number;
  waiting: string[];
}

// ---- tools

export interface ToolView {
  name: string;
  title: string;
  version: string;
  description: string;
  parameters: { properties?: Record<string, { type?: string; description?: string }>; required?: string[] };
  network: boolean;
  timeout_secs: number;
  dependencies: string[];
  packages: { name: string; version: string }[];
  builtin: boolean;
  enabled: boolean;
  network_allowed: boolean;
  ask: boolean;
}
export interface ToolsStatus {
  runtime: { version: string; sha256: string } | null;
  python_version: string;
  download_bytes: number;
  installing: boolean;
  tools: ToolView[];
  builtin_count: number;
}
export type ToolsProgress =
  | { type: "download"; done: number; total: number }
  | { type: "step"; message: string }
  | { type: "finished" }
  | { type: "failed"; error: string };
// ---- questions for the user

export type AskKind = { kind: "question" } | { kind: "tool_approval"; tool: string; arguments: Record<string, unknown> };
export type Ask = AskKind & { block: string; label: string; text: string };
export interface Pending {
  id: string;
  origin: {
    run_id: string;
    kind: "chat" | "test" | "job";
    agent_id?: string;
    job_id?: string;
    title: string;
  };
  ask: Ask;
  asked_at: number;
}
export type Answer = { kind: "text"; text: string } | { kind: "allow" } | { kind: "deny"; text: string };

// ---- jobs

export type Day = "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun";
export type Trigger =
  | { kind: "once"; at: string }
  | { kind: "every"; minutes: number }
  | { kind: "daily"; time: string; days: Day[] }
  | { kind: "folder"; path: string };
export interface Job {
  format: "chummy.job";
  version: number;
  id: string;
  name: string;
  description?: string;
  enabled: boolean;
  agent: string;
  loop?: string | null;
  message: string;
  file?: string | null;
  triggers: Trigger[];
  missed: "skip" | "run_once";
  time_limit_minutes: number;
  save_reply_to?: string | null;
  notify: "always" | "problems" | "never";
}
export type Cause =
  | { kind: "schedule"; due: number }
  | { kind: "catch_up"; due: number }
  | { kind: "manual" }
  | { kind: "file"; path: string };
export interface RunRecord {
  run_id: string;
  cause: Cause;
  status: "completed" | "failed" | "stopped" | "limit" | "missed" | "skipped";
  started_at: number;
  finished_at?: number;
  reply?: string;
  note?: string;
  llm_calls: number;
  tool_calls: number;
}
export interface JobView {
  job: Job;
  state: "running" | "queued" | "idle";
  run_id?: string;
  next_run?: number;
  last?: RunRecord;
  schedule: string[];
  watch_error?: string;
}
export interface JobsOverview {
  jobs: JobView[];
  paused: boolean;
  bad_files: { file: string; error: string }[];
}

export interface PickedWorkspace {
  token: string;
  path: string;
}

// ---------------------------------------------------------------- browser fallback

const inTauri = isTauri();

const mock = {
  settings: {
    theme: "system",
    permissions: { network: false, tools: false, confirm_online_tools: true },
    providers: [],
    default_provider: null,
    local: { device: "auto" },
    hf_token_set: false,
    agents: [],
    active_agent: null,
    tools: {},
    general: { close: "ask", notifications: true, max_job_runs: 1 },
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
      model_configured: false,
      loop_configured: false,
      agent_configured: false,
    })),
  openWindow: (window: AppWindow, section: string | null = null) =>
    call<void>("open_window", { window, section }, () => undefined),
  getSettings: () => call<Settings>("get_settings", undefined, () => mock.settings),
  setTheme: (theme: Theme) =>
    call<void>("set_theme", { theme }, () => {
      mock.settings = { ...mock.settings, theme };
      mock.listeners.forEach((cb) => cb(mock.settings));
    }),
  appInfo: () =>
    call<AppInfo>("app_info", undefined, () => ({ version: "dev", data_dir: "(browser preview)", recovery: null })),
  revealDataDir: () => call<void>("reveal_data_dir"),

  pickWorkspace: (agentId: string | null) =>
    call<PickedWorkspace | null>("pick_workspace", { agentId }, () => null),
  saveAgent: (
    agent: { id: string; name: string; loop_id: string; model: string | null },
    workspaceToken: string | null,
  ) => call<string>("save_agent", { agent, workspaceToken }),
  deleteAgent: (id: string) => call<boolean>("delete_agent", { id }),
  setActiveAgent: (id: string) => call<void>("set_active_agent", { id }),

  blockCatalog: () => call<BlockInfo[]>("block_catalog", undefined, () => []),
  loopTemplates: () => call<Template[]>("loop_templates", undefined, () => []),
  listLoops: () => call<LoopSummary[]>("list_loops", undefined, () => []),
  loadLoop: (id: string) => call<LoopFile>("load_loop", { id }),
  analyzeLoop: (file: LoopFile) => call<Analysis>("analyze_loop", { file }),
  saveLoop: (id: string | null, file: LoopFile) => call<string>("save_loop", { id, file }),
  deleteLoop: (id: string) => call<boolean>("delete_loop", { id }),
  importLoop: () => call<string | null>("import_loop", undefined, () => null),
  exportLoop: (id: string) => call<boolean>("export_loop", { id }),

  getChat: (agentId: string) => call<ChatMessage[]>("get_chat", { agentId }, () => []),
  clearChat: (agentId: string) => call<void>("clear_chat", { agentId }),
  sendMessage: (agentId: string, text: string) => call<string>("send_message", { agentId, text }),
  testRun: (file: LoopFile, agentId: string | null, message: string, overrides: Record<string, string>) =>
    call<string>("test_run", { file, agentId, message, overrides }),
  stopRun: (runId: string) => call<void>("stop_run", { runId }),
  listRuns: () => call<RunInfo[]>("list_runs", undefined, () => []),
  setPermission: (kind: "network" | "tools" | "confirm_online_tools", enabled: boolean) =>
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

  toolsStatus: () =>
    call<ToolsStatus>("tools_status", undefined, () => ({
      runtime: null,
      python_version: "3.14",
      download_bytes: 14_000_000,
      installing: false,
      tools: [],
      builtin_count: 7,
    })),
  setupTools: () => call<boolean>("setup_tools", undefined, () => false),
  setToolEnabled: (name: string, enabled: boolean) => call<void>("set_tool_enabled", { name, enabled }),
  setToolNetwork: (name: string, enabled: boolean) => call<boolean>("set_tool_network", { name, enabled }),
  setToolAsk: (name: string, ask: boolean) => call<void>("set_tool_ask", { name, ask }),
  setGeneral: (general: General) => call<void>("set_general", { general }),

  listAsks: () => call<Pending[]>("list_asks", undefined, () => []),
  answerAsk: (id: string, answer: Answer) => call<void>("answer_ask", { id, answer }),
  closeActivity: () => call<Activity>("close_activity"),
  closeChoice: (choice: CloseAction, remember: boolean) => call<void>("close_choice", { choice, remember }),

  listJobs: () => call<JobsOverview>("list_jobs", undefined, () => ({ jobs: [], paused: false, bad_files: [] })),
  saveJob: (job: Job) => call<string>("save_job", { job }),
  deleteJob: (id: string) => call<void>("delete_job", { id }),
  setJobEnabled: (id: string, enabled: boolean) => call<void>("set_job_enabled", { id, enabled }),
  runJobNow: (id: string) => call<void>("run_job_now", { id }),
  stopJob: (id: string) => call<void>("stop_job", { id }),
  jobHistory: (id: string) => call<RunRecord[]>("job_history", { id }, () => []),
  setJobsPaused: (paused: boolean) => call<void>("set_jobs_paused", { paused }),
  importJob: () => call<string | null>("import_job", undefined, () => null),
  exportJob: (id: string) => call<boolean>("export_job", { id }),
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
  onLoopsChanged: (cb: (l: LoopSummary[]) => void) => on("loops:changed", cb),
  onRunStarted: (cb: (r: RunInfo) => void) => on("run:started", cb),
  onRunEvent: (cb: (e: { run_id: string; event: FlowEvent }) => void) => on("run:event", cb),
  onRunFinished: (cb: (e: { run_id: string; agent_id: string | null; outcome: RunOutcome }) => void) =>
    on("run:finished", cb),
  onChatMessage: (
    cb: (e: { agent_id: string; message?: ChatMessage; cleared?: boolean }) => void,
  ) => on("chat:message", cb),
  onToolsChanged: (cb: (s: ToolsStatus) => void) => on("tools:changed", cb),
  onToolsProgress: (cb: (p: ToolsProgress) => void) => on("tools:progress", cb),
  onAsksChanged: (cb: (p: Pending[]) => void) => on("asks:changed", cb),
  onJobsChanged: (cb: (o: JobsOverview) => void) => on("jobs:changed", cb),
  /** The main window only: the user closed it or chose Quit. */
  onCloseRequested: (cb: (a: Activity) => void) => on("app:close-requested", cb),
  /** Another window asked this one to show a section. */
  onGoto: (cb: (section: string) => void) => on("window:goto", cb),
};
