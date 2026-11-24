/**
 * Block switches. A block's tools (and its tools' internet) follow the
 * agent's own switches by default ("inherit"); a block can only turn things
 * off ("off"), never on, so a loop from someone else can't widen what an
 * agent may do. The app-wide switches (Security and Privacy, Settings →
 * Tools) always apply on top. Older loops may say "on": it reads as "inherit".
 *
 * In a loop file, `tools` is a switch for all tools ("inherit" | "off"), or
 * `{ all, each: { tool: switch } }` when single tools differ. Left out, it
 * means "inherit". The same shape as `ToolSwitches` in crates/flow/src/blocks.rs.
 */

export type Switch = "inherit" | "off";

export interface ToolSwitches {
  all: Switch;
  each: Record<string, Switch>;
}

function asSwitch(v: unknown, fallback: Switch = "inherit"): Switch {
  if (v === "inherit" || v === "off") return v;
  if (v === "on") return "inherit";
  if (v === true) return "inherit";
  if (v === false) return "off";
  return fallback;
}

/** Reads a block's `tools` value in any of its forms. */
export function readTools(v: unknown): ToolSwitches {
  if (v == null) return { all: "inherit", each: {} };
  if (typeof v === "string" || typeof v === "boolean") return { all: asSwitch(v), each: {} };
  if (Array.isArray(v)) {
    // The old form: a list of tools, and no others.
    return { all: "off", each: Object.fromEntries(v.map((n) => [String(n), "inherit" as Switch])) };
  }
  const o = v as { all?: unknown; each?: Record<string, unknown> };
  const each: Record<string, Switch> = {};
  for (const [k, s] of Object.entries(o.each ?? {})) each[k] = asSwitch(s);
  return { all: asSwitch(o.all), each };
}

/** The value to store; `undefined` means "inherit" (leave it out). */
export function writeTools(t: ToolSwitches): unknown {
  const each = Object.fromEntries(Object.entries(t.each).filter(([, s]) => s !== t.all));
  if (!Object.keys(each).length) return t.all === "inherit" ? undefined : t.all;
  return { all: t.all, each };
}

export function switchFor(t: ToolSwitches, tool: string): Switch {
  return t.each[tool] ?? t.all;
}

export function readSwitch(v: unknown): Switch {
  return asSwitch(v);
}

/** Whether a tool is disabled in this block. */
export function isDisabled(t: ToolSwitches, tool: string): boolean {
  return switchFor(t, tool) === "off";
}

/** Disables or re-enables one tool (relative to "all"). */
export function setDisabled(t: ToolSwitches, tool: string, disabled: boolean): ToolSwitches {
  return { all: t.all, each: { ...t.each, [tool]: disabled ? "off" : "inherit" } };
}

/** Short text for a block's summary line. */
export function describeTools(v: unknown): string {
  const t = readTools(v);
  const off = Object.values(t.each).filter((s) => s === "off").length;
  const kept = Object.values(t.each).filter((s) => s !== "off").length;
  if (t.all === "off") return kept ? `Tools off but ${kept}` : "No tools";
  if (!off) return "Agent's tools";
  return `Agent's tools, ${off} disabled`;
}
