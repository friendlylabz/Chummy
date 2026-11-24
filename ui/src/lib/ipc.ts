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
import type { Generation, ModelOverrides } from "./generation";
export type { Generation, ModelOverrides } from "./generation";

// ---------------------------------------------------------------- types

export type Theme = "system" | "dark" | "light";
export type AppWindow = "editor" | "settings" | "scheduler" | "logs";
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
  /** Tokens this model may use per day; `null` means no limit. */
  daily_tokens?: number | null;
  /** Longest answer, unless a model block sets its own. */
  max_tokens?: number;
  /** Context length the server reports for the model (`null`: it doesn't say). */
  context_max?: number | null;
  /** Context length set by the user (capped at `context_max`). */
  context_tokens?: number | null;
  /** When loops make room, in tokens (`null`: 80% of the context). */
  compact_at?: number | null;
  /** "Use model within app": Chummy may use it outside loops (chat titles…). Off by default. */
  use_in_app?: boolean;
  /** Reasoning effort, sampling, penalties, seed and stop sequences (unset: the server's default). */
  generation?: Generation;
}

export const DEFAULT_CONTEXT = 8192;
export const DEFAULT_COMPACT_PERCENT = 80;
export const MIN_CONTEXT = 512;

/** The context length in use (mirrors `ProviderConfig::context_window`). */
export function contextWindow(p: Pick<ProviderConfig, "context_max" | "context_tokens">): number {
  const max = p.context_max && p.context_max > 0 ? p.context_max : null;
  const mine = p.context_tokens && p.context_tokens > 0 ? p.context_tokens : null;
  return mine != null && max != null ? Math.min(mine, max) : mine ?? max ?? DEFAULT_CONTEXT;
}

/** When loops make room (mirrors `ProviderConfig::compact_threshold`). */
export function compactThreshold(p: Pick<ProviderConfig, "context_max" | "context_tokens" | "compact_at">): number {
  const w = contextWindow(p);
  return p.compact_at && p.compact_at > 0 ? Math.min(p.compact_at, w) : Math.floor((w * DEFAULT_COMPACT_PERCENT) / 100);
}

export const DEFAULT_DAILY_TOKENS = 1_000_000;
export const DEFAULT_MAX_TOKENS = 1024;

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
  /** Tools this agent may use (each must also be on in Settings → Tools). */
  tools?: string[];
  /** Whether this agent's tools may go online (needs the app switch too). */
  network?: boolean;
  /** Per-loop block settings that replace the loop file's, by loop id. */
  loop_config?: Record<string, Overrides>;
  /** Changes to its model's settings; anything missing inherits the model's. */
  model_options?: ModelOverrides;
}

/** Block settings by block id (or `function/block`); `null` removes a key. */
export type Overrides = Record<string, Record<string, unknown>>;

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
  logging: Logging;
  sandbox: SandboxSettings;
}

export type CloseAction = "ask" | "quit" | "tray";
export interface General {
  close: CloseAction;
  notifications: boolean;
  max_job_runs: number;
  /** Format replies as Markdown. */
  markdown: boolean;
  /** Show LaTeX math in replies. */
  math: boolean;
}

export type RunLogLevel = "off" | "summary" | "standard" | "verbose";
export type SystemLogLevel = "off" | "errors" | "warnings" | "info" | "debug";
export interface Logging {
  runs: RunLogLevel;
  encrypt: boolean;
  system: SystemLogLevel;
}
export interface LogKeyStatus {
  fingerprint: string | null;
  error: string | null;
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
  /** Chosen with --data-dir instead of the default. */
  data_dir_custom: boolean;
  /** Only you can open it. */
  data_dir_locked: boolean;
  /** Windows with --data-dir: others on the computer may be able to open it. */
  data_dir_maybe_shared: boolean;
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
  /** The context length the server reports, if it does. */
  context?: number | null;
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
  /** A note for people about what the block is for. */
  comment?: string;
}
/** A free-standing note on the canvas. */
export interface Note {
  id: string;
  text: string;
  position: Position;
  width?: number;
  height?: number;
}
export interface PortRef {
  block: string;
  port: string;
}
export interface Link {
  from: PortRef;
  to: PortRef;
  /** A short label shown on the path. */
  label?: string;
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
  notes?: Note[];
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
  notes?: Note[];
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
export type Category = "start" | "think" | "you" | "exchange" | "text" | "flow" | "workspace" | "memory" | "finish";
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
  run?: RunSummary;
}
export interface RunInfo {
  run_id: string;
  agent_id: string | null;
  /** A chat run's chat, and its title. */
  chat_id?: string;
  chat_title?: string;
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
  | { type: "pane"; block: string; text: string }
  | { type: "asked"; block: string; text: string }
  | { type: "answered"; block: string }
  | ({ type: "step" } & StepLog)
  | ({ type: "model_call" } & ModelLog);

// ---------------------------------------------------------------- logs

/** A value as a log shows it (cut to a readable size). */
export interface LoggedValue {
  kind: "text" | "list" | "exchange";
  text: string;
  /** Characters in the whole value. */
  chars: number;
  /** Characters left out of `text`. */
  cut?: number;
  /** Items in a list, or messages in an exchange. */
  items?: number;
}
export interface PortLog {
  port: string;
  value: LoggedValue;
}
export type RunStepStatus = "done" | "caught" | "failed" | "stopped";
export interface StepLog {
  n: number;
  block: string;
  label: string;
  kind: string;
  function?: string;
  inputs: PortLog[];
  outputs: PortLog[];
  status: RunStepStatus;
  error?: string;
  started_at: number;
  ms: number;
}
export interface ModelLog {
  block: string;
  attempt: number;
  model?: string;
  tool_mode: string;
  tools?: string[];
  sent: LoggedValue;
  answer: LoggedValue;
  reasoning?: string;
  calls?: string[];
  started_at: number;
  ms: number;
}
export interface RunHeader {
  run_id: string;
  title: string;
  source: "chat" | "job" | "test";
  agent_id?: string;
  agent_name?: string;
  job_id?: string;
  loop_name: string;
  model?: string;
  message: string;
  started_at: number;
}
export interface RunEnd {
  ended_at: number;
  status: string;
  error?: string;
  block?: string;
  steps: number;
  llm_calls: number;
  tool_calls: number;
  replies: number;
  /** Estimated tokens the turn added to context. */
  tokens_added?: number;
  /** Estimated tokens in the latest model call's context, and its length. */
  context_tokens?: number;
  context_window?: number;
  /** The model's tokens used today, and today's allowance (none: no limit). */
  daily_used?: number;
  daily_allowed?: number | null;
}

/** One of an agent's chats. */
export interface ChatMeta {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  messages: number;
}

/** An agent's chats (newest first) and the open one. */
export interface ChatIndex {
  version: number;
  active: string | null;
  chats: ChatMeta[];
}

/** A finished run, as kept with its chat message. */
export interface RunSummary {
  run_id?: string;
  status: string;
  steps: number;
  llm_calls: number;
  tool_calls?: number;
  /** How long the run took, in ms (missing on older replies). */
  ms?: number;
  /** Estimated tokens this turn added to context. */
  tokens?: number;
  /** Estimated tokens in the context at the end, and the model's context length. */
  context_tokens?: number;
  context_window?: number;
  /** The model's tokens used today when the reply came, and today's allowance. */
  daily_used?: number;
  daily_allowed?: number | null;
}
export type LogEntry =
  | ({ type: "start" } & RunHeader)
  | ({ type: "step" } & StepLog)
  | ({ type: "model" } & ModelLog)
  | { type: "tool_call"; at: number; block: string; tool: string; arguments: string }
  | { type: "tool_result"; at: number; block: string; tool: string; text: string; is_error: boolean }
  | { type: "notice"; at: number; block: string; text: string }
  | { type: "asked"; at: number; block: string; text: string }
  | { type: "answered"; at: number; block: string }
  | { type: "reply"; at: number; block: string; text: string }
  | ({ type: "end" } & RunEnd)
  | { type: "unreadable"; lines: number; reason: string };
export interface RunListItem {
  header: RunHeader;
  end: RunEnd | null;
  bytes: number;
  day: string;
  encrypted: boolean;
  /** Why it can't be read here, if it can't. */
  locked: string | null;
  /** "completed", "stopped", "failed", "limit", "interrupted" or "running". */
  status: string;
}
export interface RunQuery {
  text?: string;
  full_text?: boolean;
  source?: string;
  status?: string;
  agent_id?: string;
  from_day?: string;
  to_day?: string;
  limit?: number;
}
export type LogLevel = "debug" | "info" | "warn" | "error";
export interface SysEntry {
  at: number;
  level: LogLevel;
  source: string;
  message: string;
  fields?: Record<string, string>;
}
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
  /** Marked "potentially unsafe" (the command tool). */
  risky: boolean;
  timeout_secs: number;
  dependencies: string[];
  packages: { name: string; version: string }[];
  builtin: boolean;
  enabled: boolean;
  network_allowed: boolean;
  ask: boolean;
  /** What it needs from the sandbox beyond Python ("programs"). */
  needs: string[];
  requires_python: string | null;
  /** Why the chosen sandbox can't run it (shown greyed out). */
  unusable: string | null;
}
export interface ToolsStatus {
  runtime: { version: string; sha256: string } | null;
  python_version: string;
  python_bundled: boolean;
  installing: boolean;
  tools: ToolView[];
  builtin_count: number;
  sandbox: SandboxHealth;
}

// ---- sandbox

export type SandboxKind = "native" | "container" | "openshell";
export type ContainerEngine = "docker" | "podman";
export interface SandboxSettings {
  kind: SandboxKind;
  container: {
    engine: ContainerEngine;
    /** Empty: Chummy finds it. Set only through the native picker. */
    program: string;
    image: string;
    /** The image ID Chummy runs, once you got the image. */
    approved_id: string | null;
    memory_mb: number;
    cpus: number;
  };
  openshell: { program: string; image: string };
}
export interface SandboxHealth {
  kind: SandboxKind;
  ready: boolean;
  message: string;
  python: string | null;
  programs: boolean;
  details: string[];
  /** 0: not checked yet. */
  checked_at: number;
}
export interface SandboxStatus {
  settings: SandboxSettings;
  health: SandboxHealth;
  checking: boolean;
  kinds: { kind: SandboxKind; label: string; programs: boolean }[];
  default_image: string;
}
export type ProgramFor = "container" | "openshell";
export type ToolsProgress =
  | { type: "step"; message: string }
  | { type: "finished" }
  | { type: "failed"; error: string };
// ---- questions for the user

export type AskKind =
  | { kind: "question" }
  | { kind: "tool_approval"; tool: string; arguments: Record<string, unknown> }
  | { kind: "budget"; model: string; used: number; limit: number; extra: number };

export interface AgentLoopConfig {
  loop_id: string;
  file: LoopFile;
  overrides: Overrides;
  /** Overridden blocks the loop no longer has. */
  missing: string[];
}
export interface UsageView {
  provider_id: string;
  name: string;
  used: number;
  extra: number;
  calls: number;
  limit: number | null;
}
export type Ask = AskKind & { block: string; label: string; text: string };
export interface Pending {
  id: string;
  origin: {
    run_id: string;
    kind: "chat" | "test" | "job";
    agent_id?: string;
    chat_id?: string;
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
    general: { close: "ask", notifications: true, max_job_runs: 1, markdown: true, math: true },
    logging: { runs: "verbose", encrypt: true, system: "info" },
    sandbox: {
      kind: "native",
      container: { engine: "docker", program: "", image: "python:3.14-slim", approved_id: null, memory_mb: 1024, cpus: 2 },
      openshell: { program: "", image: "" },
    },
  } as Settings,
  listeners: new Set<(s: Settings) => void>(),
};

function mockHealth(): SandboxHealth {
  return {
    kind: "native",
    ready: true,
    message: "Tools run in WebAssembly, built into Chummy.",
    python: "3.14.7",
    programs: false,
    details: ["Python 3.14.7 for WebAssembly"],
    checked_at: Date.now(),
  };
}

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
    call<AppInfo>("app_info", undefined, () => ({
      version: "dev",
      data_dir: "(browser preview)",
      data_dir_custom: false,
      data_dir_locked: true,
      data_dir_maybe_shared: false,
      recovery: null,
    })),
  revealDataDir: () => call<void>("reveal_data_dir"),

  pickWorkspace: (agentId: string | null) =>
    call<PickedWorkspace | null>("pick_workspace", { agentId }, () => null),
  saveAgent: (
    agent: { id: string; name: string; loop_id: string; model: string | null },
    workspaceToken: string | null,
  ) => call<string>("save_agent", { agent, workspaceToken }),
  deleteAgent: (id: string) => call<boolean>("delete_agent", { id }),
  setActiveAgent: (id: string) => call<void>("set_active_agent", { id }),
  setAgentAccess: (agentId: string, tools: string[], network: boolean) =>
    call<{ tools: string[]; network: boolean }>("set_agent_access", { agentId, tools, network }, () => ({
      tools,
      network,
    })),
  agentLoopConfig: (agentId: string) => call<AgentLoopConfig>("agent_loop_config", { agentId }),
  setAgentBlockConfig: (agentId: string, block: string, params: Record<string, unknown> | null) =>
    call<void>("set_agent_block_config", { agentId, block, params }),
  /** Saves an agent's changes to its model's settings (`{}`: inherit all). */
  setAgentModelOptions: (agentId: string, options: ModelOverrides) =>
    call<void>("set_agent_model_options", { agentId, options }),
  exportAgentLoopConfig: (agentId: string) =>
    call<boolean>("export_agent_loop_config", { agentId }, () => false),
  importAgentLoopConfig: (agentId: string) =>
    call<number | null>("import_agent_loop_config", { agentId }, () => null),
  usageToday: () => call<UsageView[]>("usage_today", undefined, () => []),

  blockCatalog: () => call<BlockInfo[]>("block_catalog", undefined, () => []),
  loopTemplates: () => call<Template[]>("loop_templates", undefined, () => []),
  listLoops: () => call<LoopSummary[]>("list_loops", undefined, () => []),
  loadLoop: (id: string) => call<LoopFile>("load_loop", { id }),
  analyzeLoop: (file: LoopFile) => call<Analysis>("analyze_loop", { file }),
  saveLoop: (id: string | null, file: LoopFile) => call<string>("save_loop", { id, file }),
  /** Adds the recommended loop (Chummy assistant), or finds it. Returns its id. */
  addRecommendedLoop: () => call<string>("add_recommended_loop", undefined, () => "chummy-assistant"),
  deleteLoop: (id: string) => call<boolean>("delete_loop", { id }),
  importLoop: () => call<string | null>("import_loop", undefined, () => null),
  exportLoop: (id: string) => call<boolean>("export_loop", { id }),

  listChats: (agentId: string) =>
    call<ChatIndex>("list_chats", { agentId }, () => {
      const c = { id: "c-demo", title: "New chat", created_at: 0, updated_at: 0, messages: 0 };
      return { version: 1, active: c.id, chats: [c] };
    }),
  newChat: (agentId: string) => call<ChatMeta>("new_chat", { agentId }),
  setActiveChat: (agentId: string, chatId: string) => call<void>("set_active_chat", { agentId, chatId }),
  deleteChat: (agentId: string, chatId: string) => call<boolean>("delete_chat", { agentId, chatId }),
  deleteChatMessage: (agentId: string, chatId: string, messageId: string) =>
    call<void>("delete_chat_message", { agentId, chatId, messageId }),
  restoreChatMessage: (agentId: string, chatId: string, messageId: string) =>
    call<void>("restore_chat_message", { agentId, chatId, messageId }),
  getChat: (agentId: string, chatId: string) => call<ChatMessage[]>("get_chat", { agentId, chatId }, () => []),
  sendMessage: (agentId: string, chatId: string, text: string) =>
    call<string>("send_message", { agentId, chatId, text }),
  testRun: (file: LoopFile, agentId: string | null, message: string, overrides: Record<string, string>) =>
    call<string>("test_run", { file, agentId, message, overrides }),
  stopRun: (runId: string) => call<void>("stop_run", { runId }),
  searchRuns: (query: RunQuery) => call<RunListItem[]>("search_runs", { query }, () => []),
  readRunLog: (runId: string) => call<LogEntry[]>("read_run_log", { runId }, () => []),
  systemLogDays: () => call<string[]>("system_log_days", undefined, () => []),
  readSystemLog: (day: string) => call<SysEntry[]>("read_system_log", { day }, () => []),
  revealLogs: () => call<void>("reveal_logs"),
  setLogging: (logging: Logging) => call<Logging>("set_logging", { logging }, () => logging),
  logKeyStatus: () =>
    call<LogKeyStatus>("log_key_status", undefined, () => ({ fingerprint: null, error: null })),
  exportLogKey: (passphrase: string) => call<boolean>("export_log_key", { passphrase }, () => false),
  importLogKey: (passphrase: string) => call<string | null>("import_log_key", { passphrase }, () => null),
  /** Opens a link from a reply, after Rust asks. */
  openLink: (url: string) => call<void>("open_link", { url }),
  listRuns: () => call<RunInfo[]>("list_runs", undefined, () => []),
  /** What a running run's Pane output blocks have shown, newest first. */
  runPane: (runId: string) => call<string[]>("run_pane", { runId }, () => []),
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
      python_version: "3.14.10",
      python_bundled: true,
      installing: false,
      tools: [],
      builtin_count: 11,
      sandbox: mockHealth(),
    })),
  setupTools: () => call<boolean>("setup_tools", undefined, () => false),
  setToolEnabled: (name: string, enabled: boolean) => call<boolean>("set_tool_enabled", { name, enabled }),
  setToolNetwork: (name: string, enabled: boolean) => call<boolean>("set_tool_network", { name, enabled }),
  /** Turning it off for a potentially unsafe tool asks natively. False: cancelled. */
  setToolAsk: (name: string, ask: boolean) => call<boolean>("set_tool_ask", { name, ask }, () => true),

  sandboxStatus: () =>
    call<SandboxStatus>("sandbox_status", undefined, () => ({
      settings: mock.settings.sandbox,
      health: mockHealth(),
      checking: false,
      kinds: [
        { kind: "native", label: "Native (WebAssembly)", programs: false },
        { kind: "container", label: "Container (Docker or Podman)", programs: true },
        { kind: "openshell", label: "NVIDIA OpenShell", programs: true },
      ],
      default_image: "python:3.14-slim",
    })),
  checkSandbox: () => call<SandboxHealth>("check_sandbox", undefined, mockHealth),
  /** Asks natively when switching to Container or OpenShell. False: cancelled. */
  setSandboxKind: (kind: SandboxKind) => call<boolean>("set_sandbox_kind", { kind }, () => true),
  setContainerOptions: (choice: { engine: ContainerEngine; image: string; memory_mb: number; cpus: number }) =>
    call<void>("set_container_options", { choice }),
  /** Downloads the image (pull) or uses the local copy; asks first. */
  getContainerImage: (pull: boolean) => call<boolean>("get_container_image", { pull }, () => false),
  setOpenshellImage: (image: string) => call<void>("set_openshell_image", { image }),
  chooseSandboxProgram: (which: ProgramFor) => call<boolean>("choose_sandbox_program", { which }, () => false),
  clearSandboxProgram: (which: ProgramFor) => call<void>("clear_sandbox_program", { which }),
  removeOpenshellSandboxes: () => call<number | null>("remove_openshell_sandboxes", undefined, () => null),
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
  onChatMessage: (cb: (e: { agent_id: string; chat_id: string; message: ChatMessage }) => void) =>
    on("chat:message", cb),
  onChatMessageDeleted: (cb: (e: { agent_id: string; chat_id: string; message_id: string }) => void) =>
    on("chat:message_deleted", cb),
  onChatMessageRestored: (
    cb: (e: { agent_id: string; chat_id: string; message: ChatMessage; position: number }) => void,
  ) => on("chat:message_restored", cb),
  onChatsChanged: (cb: (e: { agent_id: string; index: ChatIndex }) => void) => on("chats:changed", cb),
  onToolsChanged: (cb: (s: ToolsStatus) => void) => on("tools:changed", cb),
  onSandboxChanged: (cb: (s: SandboxStatus) => void) => on("sandbox:changed", cb),
  onToolsProgress: (cb: (p: ToolsProgress) => void) => on("tools:progress", cb),
  onAsksChanged: (cb: (p: Pending[]) => void) => on("asks:changed", cb),
  onJobsChanged: (cb: (o: JobsOverview) => void) => on("jobs:changed", cb),
  /** The main window only: the user closed it or chose Quit. */
  onCloseRequested: (cb: (a: Activity) => void) => on("app:close-requested", cb),
  /** Another window asked this one to show a section. */
  onGoto: (cb: (section: string) => void) => on("window:goto", cb),
  onSystemLog: (cb: (e: SysEntry) => void) => on("log:system", cb),
};
