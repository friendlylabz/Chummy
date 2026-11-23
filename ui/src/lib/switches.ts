/**
 * Block switches that inherit. A block's tools (and its tools' internet) are
 * "inherit" by default: the agent's own switches decide. "on" turns something
 * on for that block even when the agent's switch is off; "off" never. The
 * app-wide switches (Permissions, Settings → Tools) always apply on top.
 *
 * In a loop file, `tools` is a switch for all tools ("inherit" | "on" | "off"),
 * or `{ all, each: { tool: switch } }` when single tools differ. Left out, it
 * means "inherit". The same shape as `ToolSwitches` in crates/flow/src/blocks.rs.
 */

export type Switch = "inherit" | "on" | "off";

export interface ToolSwitches {
  all: Switch;
  each: Record<string, Switch>;
}

function asSwitch(v: unknown, fallback: Switch = "inherit"): Switch {
  if (v === "inherit" || v === "on" || v === "off") return v;
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

/** Short text for a block's summary line. */
export function describeTools(v: unknown): string {
  const t = readTools(v);
  const n = Object.keys(t.each).length;
  const base = t.all === "inherit" ? "Agent's tools" : t.all === "on" ? "All tools" : n ? "" : "No tools";
  if (!n) return base;
  const on = Object.values(t.each).filter((s) => s !== "off").length;
  if (t.all === "off") return `${on} tool${on === 1 ? "" : "s"}`;
  return `${base}, ${n} changed`;
}

export const switchLabels: Record<Switch, string> = {
  inherit: "Same as the agent",
  on: "On",
  off: "Off",
};
