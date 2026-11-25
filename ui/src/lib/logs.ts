/**
 * Turning run logs into a timeline people can read, sort and filter. Used
 * by the Logs window and by the step view under each chat message.
 */
import { commands, type BlockInfo, type Category, type FlowEvent, type LogEntry, type RunEnd, type RunHeader } from "$lib/ipc";

/** What kind of thing a timeline row is, for filtering. */
export type Group = "model" | "tools" | "you" | "replies" | "notices" | "blocks";

export const groupLabel: Record<Group, string> = {
  model: "Model calls",
  tools: "Tools",
  you: "Questions",
  replies: "Replies",
  notices: "Notices",
  blocks: "Block steps",
};

export interface Item {
  /** Index in the log, for stable keys and order. */
  i: number;
  entry: LogEntry;
  group: Group;
  /** Unix ms. */
  at: number;
  /** Function nesting: 0 for the loop's own blocks. */
  depth: number;
  block: string;
  title: string;
  /** A one-line preview. */
  preview: string;
  ms?: number;
  failed: boolean;
  /** For block steps: the block's category. */
  category?: Category;
  /** Lower-case text searched by the filter. */
  haystack: string;
}

export interface Timeline {
  header: RunHeader | null;
  end: RunEnd | null;
  items: Item[];
  /** Lines that couldn't be read, and why. */
  unreadable: { lines: number; reason: string } | null;
}

let catalog: Promise<BlockInfo[]> | null = null;
/** The block catalog, fetched once per window. */
export function blockCatalog(): Promise<BlockInfo[]> {
  catalog ??= commands.blockCatalog().catch(() => []);
  return catalog;
}

const oneLine = (s: string, n = 140) => {
  const t = s.replace(/\s+/g, " ").trim();
  return t.length > n ? t.slice(0, n) + "…" : t;
};

function depthOf(block: string) {
  return block ? block.split("/").length - 1 : 0;
}

/** Builds the timeline from a run log's entries. */
export function timeline(entries: LogEntry[], blocks: BlockInfo[] = []): Timeline {
  let header: RunHeader | null = null;
  let end: RunEnd | null = null;
  let unreadable: Timeline["unreadable"] = null;
  const info = new Map(blocks.map((b) => [b.kind, b]));
  const items: Item[] = [];
  entries.forEach((e, i) => {
    const base = { i, entry: e, failed: false };
    switch (e.type) {
      case "start":
        header = e;
        return;
      case "end":
        end = e;
        return;
      case "unreadable":
        unreadable = { lines: e.lines, reason: e.reason };
        return;
      case "step": {
        const b = info.get(e.kind);
        const kind = e.function ? `Function: ${e.function}` : (b?.label ?? e.kind);
        const firstOut = e.outputs.find((o) => o.value.text.trim()) ?? e.outputs[0];
        const preview = e.error
          ? e.error
          : firstOut
            ? `${firstOut.port} → ${oneLine(firstOut.value.text)}`
            : e.inputs[0]
              ? `${e.inputs[0].port} ← ${oneLine(e.inputs[0].value.text)}`
              : "";
        items.push({
          ...base,
          group: "blocks",
          at: e.started_at,
          depth: depthOf(e.block),
          block: e.block,
          title: e.label || e.block,
          preview,
          ms: e.ms,
          failed: e.status === "failed" || e.status === "caught",
          category: b?.category,
          haystack: [e.label, e.block, kind, e.error ?? "", ...e.inputs.map((p) => p.value.text), ...e.outputs.map((p) => p.value.text)]
            .join("\n")
            .toLowerCase(),
        });
        return;
      }
      case "model":
        items.push({
          ...base,
          group: "model",
          at: e.started_at,
          depth: depthOf(e.block),
          block: e.block,
          title: e.calls?.length ? `Model asked for ${e.calls.length === 1 ? "a tool" : `${e.calls.length} tools`}` : "Model answered",
          preview: e.calls?.length ? oneLine(e.calls.join(", ")) : oneLine(e.answer.text) || "(empty answer)",
          ms: e.ms,
          haystack: [e.block, e.sent.text, e.answer.text, e.reasoning ?? "", ...(e.calls ?? [])].join("\n").toLowerCase(),
        });
        return;
      case "tool_call":
        items.push({
          ...base,
          group: "tools",
          at: e.at,
          depth: depthOf(e.block),
          block: e.block,
          title: `Tool call: ${e.tool}`,
          preview: oneLine(e.arguments),
          haystack: [e.block, e.tool, e.arguments].join("\n").toLowerCase(),
        });
        return;
      case "tool_result":
        items.push({
          ...base,
          group: "tools",
          at: e.at,
          depth: depthOf(e.block),
          block: e.block,
          title: `${e.is_error ? "Tool error" : "Tool result"}: ${e.tool}`,
          preview: oneLine(e.text),
          failed: e.is_error,
          haystack: [e.block, e.tool, e.text].join("\n").toLowerCase(),
        });
        return;
      case "notice":
        items.push({
          ...base,
          group: "notices",
          at: e.at,
          depth: depthOf(e.block),
          block: e.block,
          title: "Notice",
          preview: oneLine(e.text),
          failed: /^failed/i.test(e.text),
          haystack: [e.block, e.text].join("\n").toLowerCase(),
        });
        return;
      case "asked":
        items.push({
          ...base,
          group: "you",
          at: e.at,
          depth: depthOf(e.block),
          block: e.block,
          title: "Asked you",
          preview: oneLine(e.text),
          haystack: [e.block, e.text].join("\n").toLowerCase(),
        });
        return;
      case "answered":
        items.push({ ...base, group: "you", at: e.at, depth: depthOf(e.block), block: e.block, title: "Wait ended", preview: "", haystack: e.block.toLowerCase() });
        return;
      case "reply":
        items.push({
          ...base,
          group: "replies",
          at: e.at,
          depth: depthOf(e.block),
          block: e.block,
          title: "Reply",
          preview: oneLine(e.text),
          haystack: [e.block, e.text].join("\n").toLowerCase(),
        });
        return;
    }
  });
  // Steps are written when they finish; show everything in start order.
  items.sort((a, b) => a.at - b.at || rank(a) - rank(b) || a.i - b.i);
  return { header, end, items, unreadable };
}

/** At the same moment, a step comes first (it started first), in step order. */
function rank(x: Item) {
  return x.entry.type === "step" ? x.entry.n : 1e9;
}

/** The log entry a live run event becomes (as Rust writes it). */
export function entryOf(e: FlowEvent): LogEntry | null {
  const at = Date.now();
  switch (e.type) {
    case "step": {
      const { type: _t, ...rest } = e;
      return { type: "step", ...rest };
    }
    case "model_call": {
      const { type: _t, ...rest } = e;
      return { type: "model", ...rest };
    }
    case "tool_call":
      return { type: "tool_call", at, block: e.block, tool: e.tool, arguments: e.arguments };
    case "tool_result":
      return { type: "tool_result", at, block: e.block, tool: e.tool, text: e.preview, is_error: e.is_error };
    case "notice":
      return { type: "notice", at, block: e.block, text: e.text };
    case "asked":
      return { type: "asked", at, block: e.block, text: e.text };
    case "answered":
      return { type: "answered", at, block: e.block };
    case "reply":
      return { type: "reply", at, block: e.block, text: e.text };
    default:
      return null;
  }
}

export function seconds(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  const m = Math.floor(ms / 60_000);
  return `${m} min ${Math.round((ms % 60_000) / 1000)} s`;
}

/** Time since the run started, e.g. "+12.4s". */
export function offset(at: number, start: number): string {
  const d = Math.max(0, at - start) / 1000;
  return d < 60 ? `+${d.toFixed(1)}s` : `+${Math.floor(d / 60)}m${String(Math.round(d % 60)).padStart(2, "0")}s`;
}

export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function dateTime(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export const statusLabel: Record<string, string> = {
  completed: "Completed",
  stopped: "Stopped",
  failed: "Failed",
  limit: "Hit a limit",
  paused: "Paused",
  interrupted: "Interrupted",
  running: "Running",
};
