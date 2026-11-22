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

/**
 * Light formatting for agent replies: **bold** and `code`. Everything is
 * escaped first, so model output can never inject markup.
 */
export function richText(s: string): string {
  const esc = s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  return esc
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*\n][^\n]*?)\*\*/g, "<strong>$1</strong>");
}
