/**
 * Converting between the loop file format and the canvas (xyflow nodes and
 * edges). The loop file is what's saved; the canvas is how it's edited.
 *
 * - each block is a node of type "block" whose id is the block id;
 * - each link is an edge whose handles are the port names.
 */
import type { Edge, Node } from "@xyflow/svelte";
import type { Block, BlockInfo, Category, Link, LoopFile, PortSpec } from "$lib/ipc";

export interface BlockData extends Record<string, unknown> {
  block: Block;
  inputs: PortSpec[];
  outputs: PortSpec[];
  issues: string[];
  /** Live state during a test run. */
  state?: "waiting" | "running" | "done" | "failed";
  output?: string;
}

export type BlockNode = Node<BlockData, "block">;

/** Deep copy that also works on Svelte state proxies (structuredClone doesn't). */
export function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v));
}

export function edgeId(l: Link): string {
  return `${l.from.block}:${l.from.port}->${l.to.block}:${l.to.port}`;
}

export function toCanvas(file: LoopFile): { nodes: BlockNode[]; edges: Edge[] } {
  const nodes: BlockNode[] = file.blocks.map((b) => ({
    id: b.id,
    type: "block",
    position: { ...b.position },
    data: { block: clone(b), inputs: [], outputs: [], issues: [] },
  }));
  const edges: Edge[] = file.links.map((l) => ({
    id: edgeId(l),
    source: l.from.block,
    sourceHandle: l.from.port,
    target: l.to.block,
    targetHandle: l.to.port,
  }));
  return { nodes, edges };
}

/** Builds the loop file from the canvas plus the loop's own fields. */
export function toFile(meta: LoopFile, nodes: BlockNode[], edges: Edge[]): LoopFile {
  return {
    ...meta,
    blocks: nodes.map((n) => ({
      ...n.data.block,
      position: { x: Math.round(n.position.x), y: Math.round(n.position.y) },
    })),
    links: edges
      .filter((e) => e.sourceHandle && e.targetHandle)
      .map((e) => ({
        from: { block: e.source, port: e.sourceHandle! },
        to: { block: e.target, port: e.targetHandle! },
      })),
  };
}

/** A key that changes only when the loop's structure (not positions) changes. */
export function structureKey(meta: LoopFile, nodes: BlockNode[], edges: Edge[]): string {
  return JSON.stringify([
    meta.limits,
    nodes.map((n) => [n.id, n.data.block.type, n.data.block.label, n.data.block.params]),
    edges.map((e) => [e.source, e.sourceHandle, e.target, e.targetHandle]),
  ]);
}

export function newBlockId(type: string, taken: Set<string>): string {
  let n = 1;
  while (taken.has(`${type}-${n}`)) n++;
  return `${type}-${n}`;
}

export function newBlock(info: BlockInfo, id: string, x: number, y: number): Block {
  return {
    id,
    type: info.kind,
    label: info.label,
    position: { x: Math.round(x), y: Math.round(y) },
    params: clone(info.default_params),
  };
}

export const categoryLabel: Record<Category, string> = {
  start: "Start",
  think: "Think",
  text: "Text",
  flow: "Flow",
  workspace: "Workspace",
  finish: "Finish",
};

/** One hue per category, used for the dot on each block. */
export const categoryColor: Record<Category, string> = {
  start: "#6fb3f2",
  think: "var(--accent)",
  text: "#a78bfa",
  flow: "#e8c16d",
  workspace: "#7fd1a4",
  finish: "#ef8fa8",
};

/** One-line summary shown on a block. */
export function summary(b: Block): string {
  const p = b.params as Record<string, unknown>;
  const s = (v: unknown) => (typeof v === "string" ? v : "");
  switch (b.type) {
    case "llm":
      return s(p.prompt).replace(/\s+/g, " ").trim();
    case "fixed":
      return s(p.text).replace(/\s+/g, " ").trim() || "(empty)";
    case "combine":
      return s(p.format).replace(/\s+/g, " ").trim();
    case "regex":
    case "capture":
    case "branch":
      return s(p.pattern);
    case "history":
      return `Last ${p.max_messages ?? 10} messages`;
    case "split":
      return `${p.chunk_size ?? 2000} characters`;
    case "join":
      return `Separator ${JSON.stringify(s(p.separator) || "\n")}`;
    case "write_file":
      return p.append ? "Append" : "Replace";
    default:
      return "";
  }
}

export function blankLoop(name = "New loop"): LoopFile {
  return {
    format: "chummy.loop",
    version: 1,
    name,
    description: "",
    limits: { max_steps: 200, max_llm_calls: 20 },
    blocks: [],
    links: [],
  };
}
