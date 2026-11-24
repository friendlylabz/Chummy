/** Small display helpers. */

export function bytes(n: number | null | undefined): string {
  if (n == null) return "—";
  const gb = n / 1e9;
  if (gb >= 10) return `${gb.toFixed(0)} GB`;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  const mb = n / 1e6;
  if (mb >= 1) return `${mb.toFixed(0)} MB`;
  return `${Math.max(1, Math.round(n / 1e3))} KB`;
}

export function params(n: number | null | undefined): string {
  if (!n) return "—";
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e10 ? 0 : 1)}B`;
  return `${Math.round(n / 1e6)}M`;
}

export function context(n: number): string {
  return n >= 1024 ? `${Math.round(n / 1024)}K` : `${n}`;
}

export const precisionLabel: Record<string, string> = {
  half: "16-bit",
  int8: "8-bit",
  int4: "4-bit",
};

export function duration(seconds: number): string {
  if (!isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return `${Math.ceil(seconds)} s`;
  if (seconds < 3600) return `${Math.ceil(seconds / 60)} min`;
  return `${(seconds / 3600).toFixed(1)} h`;
}

/**
 * A stopwatch reading to a tenth of a second: "4.2s" under a minute,
 * "1:05.3" (minutes:seconds.tenths) after, "1:02:05.3" past an hour.
 */
export function stopwatch(ms: number): string {
  const tenths = Math.max(0, Math.floor(ms / 100));
  const t = tenths % 10;
  const totalS = Math.floor(tenths / 10);
  const s = totalS % 60;
  const m = Math.floor(totalS / 60) % 60;
  const h = Math.floor(totalS / 3600);
  if (!h && !m) return `${s}.${t}s`;
  const ss = String(s).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}.${t}` : `${m}:${ss}.${t}`;
}

export function date(unix: number): string {
  return new Date(unix * 1000).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

export const kindLabel: Record<string, string> = {
  open_ai_compatible: "OpenAI-compatible",
  open_ai: "OpenAI",
  ollama: "Ollama",
  anthropic: "Anthropic",
  gemini: "Gemini",
  local: "Built-in engine",
};
