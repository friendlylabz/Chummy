// Generation settings (reasoning effort, sampling, penalties, seed, stop
// sequences) and an agent's model overrides. Mirrors crates/llm/src/generation.rs.

import type { ProviderKind } from "./ipc";

export type ReasoningEffort = "none" | "minimal" | "low" | "medium" | "high";

/** Optional settings; missing means "not sent" (the server's default). */
export interface Generation {
  reasoning_effort?: ReasoningEffort | null;
  top_p?: number | null;
  top_k?: number | null;
  min_p?: number | null;
  presence_penalty?: number | null;
  frequency_penalty?: number | null;
  repeat_penalty?: number | null;
  seed?: number | null;
  stop?: string[];
}

export type Param = Exclude<keyof Generation, never>;

/**
 * An agent's changes to its model's settings. For each key: missing =
 * inherit the model's; `null` = don't send it; a value = use that.
 * (`max_tokens` is never null; `stop: []` = send none.)
 */
export interface ModelOverrides extends Generation {
  temperature?: number | null;
  max_tokens?: number;
}

export const EFFORTS: { value: ReasoningEffort; label: string }[] = [
  { value: "none", label: "None (don't think)" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

export const MAX_STOP = 4;
export const MAX_SEED = 2 ** 53 - 1;

export interface ParamInfo {
  key: Param;
  label: string;
  /** "effort": a choice; "stop": a list; otherwise a number. */
  type: "effort" | "float" | "int" | "stop";
  min?: number;
  max?: number;
  placeholder?: string;
  help: string;
}

export const PARAMS: ParamInfo[] = [
  {
    key: "reasoning_effort",
    label: "Reasoning effort",
    type: "effort",
    help: "How long a reasoning model thinks before it answers. Higher is slower and uses more tokens; small models often do fine on low.",
  },
  { key: "top_p", label: "Top P", type: "float", min: 0, max: 1, placeholder: "e.g. 0.9", help: "Only pick from the likeliest words that add up to this share." },
  { key: "top_k", label: "Top K", type: "int", min: 1, max: 1000, placeholder: "e.g. 40", help: "Only pick from this many of the likeliest words." },
  { key: "min_p", label: "Min P", type: "float", min: 0, max: 1, placeholder: "e.g. 0.05", help: "Skip words less likely than this share of the likeliest one. Good with small local models." },
  { key: "presence_penalty", label: "Presence penalty", type: "float", min: -2, max: 2, placeholder: "e.g. 0.5", help: "Above 0 nudges toward new words and topics." },
  { key: "frequency_penalty", label: "Frequency penalty", type: "float", min: -2, max: 2, placeholder: "e.g. 0.3", help: "Above 0 nudges away from words the more they've been used." },
  { key: "repeat_penalty", label: "Repeat penalty", type: "float", min: 0, max: 10, placeholder: "e.g. 1.1", help: "Above 1 discourages repeating itself (llama.cpp, LM Studio, vLLM)." },
  { key: "seed", label: "Seed", type: "int", min: 0, max: MAX_SEED, placeholder: "e.g. 42", help: "A fixed seed makes answers repeatable, where the server supports it." },
  { key: "stop", label: "Stop sequences", type: "stop", help: `Stop the answer when one of these appears. One per line, at most ${MAX_STOP}.` },
];

/** Every setting a model call takes, including temperature and the longest answer. */
export type SettingKey = keyof ModelOverrides;
export type SettingInfo = Omit<ParamInfo, "key"> & { key: SettingKey; required?: boolean };

export const TEMPERATURE: SettingInfo = {
  key: "temperature",
  label: "Temperature",
  type: "float",
  min: 0,
  max: 2,
  placeholder: "e.g. 0.3",
  help: "Lower is more predictable. Small models often do best around 0.2–0.4.",
};
export const MAX_TOKENS: SettingInfo = {
  key: "max_tokens",
  label: "Longest answer (tokens)",
  type: "int",
  min: 16,
  max: 32768,
  placeholder: "e.g. 2048",
  help: "Small models often ramble: a lower limit keeps answers short.",
  required: true,
};
/** All of them, in the order they're shown everywhere. */
export const SETTINGS: SettingInfo[] = [TEMPERATURE, MAX_TOKENS, ...PARAMS];

/** A setting's value in a model connection (null: not sent). */
export function modelValue(m: { temperature?: number | null; max_tokens?: number; generation?: Generation } | undefined, k: SettingKey): unknown {
  if (!m) return undefined;
  if (k === "temperature") return m.temperature ?? null;
  if (k === "max_tokens") return m.max_tokens;
  return (m.generation ?? {})[k as Param] ?? null;
}

/**
 * A model block's settings as overrides: `temperature` and `max_tokens` are
 * block params of their own, the rest live in its `generation` object.
 */
export function blockOverrides(params: Record<string, unknown>): ModelOverrides {
  const o: Record<string, unknown> = { ...((params.generation as object | undefined) ?? {}) };
  if (params.temperature != null) o.temperature = params.temperature;
  if (params.max_tokens != null) o.max_tokens = params.max_tokens;
  return o as ModelOverrides;
}

/** The block params for some overrides (`undefined` removes a param). */
export function blockParams(o: ModelOverrides): { temperature: unknown; max_tokens: unknown; generation: unknown } {
  const { temperature, max_tokens, ...rest } = o;
  return {
    temperature: temperature ?? undefined,
    max_tokens: max_tokens ?? undefined,
    generation: Object.keys(rest).length ? rest : undefined,
  };
}

/** Whether a kind of server takes a setting: a reason when it doesn't (mirrors Rust `support`). */
export function unsupported(kind: ProviderKind, key: Param | SettingKey): string | null {
  if (key === "temperature" || key === "max_tokens") return null;
  switch (kind) {
    case "open_ai_compatible":
    case "ollama":
      return null;
    case "local":
      return "The built-in engine doesn't use this yet.";
    case "open_ai":
      return key === "top_k" || key === "min_p" || key === "repeat_penalty" ? "OpenAI doesn't accept this." : null;
    case "anthropic":
      if (key === "reasoning_effort")
        return "Not yet for Anthropic: its thinking has to be passed back between tool calls, which Chummy doesn't do yet.";
      return key === "top_p" || key === "top_k" || key === "stop" ? null : "Anthropic doesn't accept this.";
    case "gemini":
      return key === "min_p" || key === "repeat_penalty" ? "Gemini doesn't accept this." : null;
  }
}

/**
 * Reads what was typed for a setting: `{ value }` (null for an empty box,
 * meaning not set) or `{ error }`.
 */
export function parseParam(info: Pick<ParamInfo, "type" | "min" | "max" | "label">, text: string): { value: unknown } | { error: string } {
  const t = text.trim();
  if (info.type === "effort") return { value: t === "" ? null : t };
  if (info.type === "stop") {
    const list = text
      .split("\n")
      .map((s) => s.replace(/\r$/, ""))
      .filter((s) => s.trim() !== "");
    if (list.length > MAX_STOP) return { error: `At most ${MAX_STOP} stop sequences.` };
    if (list.some((s) => s.length > 100)) return { error: "Each stop sequence can be at most 100 characters." };
    return { value: list };
  }
  if (t === "") return { value: null };
  const n = Number(t);
  const range = `${info.label} must be between ${(info.min ?? 0).toLocaleString("en-US")} and ${(info.max ?? 0).toLocaleString("en-US")}.`;
  if (!Number.isFinite(n)) return { error: range };
  if (info.type === "int" && !Number.isInteger(n)) return { error: `${info.label} must be a whole number.` };
  if ((info.min !== undefined && n < info.min) || (info.max !== undefined && n > info.max)) return { error: range };
  return { value: n };
}

/** A setting's value as text for an input box. */
export function showParam(type: ParamInfo["type"], v: unknown): string {
  if (v === null || v === undefined) return "";
  if (type === "stop") return (v as string[]).join("\n");
  return String(v);
}

/** A setting's value in words ("not sent" when unset). */
export function describe(type: ParamInfo["type"], v: unknown): string {
  if (v === null || v === undefined || (Array.isArray(v) && !v.length)) return "not sent";
  if (type === "effort") return EFFORTS.find((e) => e.value === v)?.label.toLowerCase() ?? String(v);
  if (type === "stop") return (v as string[]).map((s) => JSON.stringify(s)).join(", ");
  return String(v);
}
