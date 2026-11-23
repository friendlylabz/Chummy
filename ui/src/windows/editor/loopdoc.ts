/**
 * Converting between the loop file format and the canvas (xyflow nodes and
 * edges). The loop file is what's saved; the canvas is how it's edited.
 *
 * - each block is a node of type "block" whose id is the block id;
 * - each link is an edge whose handles are the port names;
 * - each note is a node of type "note" whose id is `note:` + the note id.
 */
import type { Edge, Node } from "@xyflow/svelte";
import { describeTools } from "$lib/switches";
import type { Block, BlockInfo, BlockPorts, FunctionDef, Link, LoopFile, Note, PortSpec } from "$lib/ipc";

/** The output a block uses for a failure it can recover from. */
export const FAILED = "error";

export interface BlockData extends Record<string, unknown> {
  block: Block;
  inputs: PortSpec[];
  outputs: PortSpec[];
  issues: string[];
  /** Live state during a test run. */
  state?: "waiting" | "running" | "done" | "failed";
  output?: string;
  /** Inside a function: ports that are the function's inputs and outputs. */
  exposedIn?: string[];
  exposedOut?: string[];
}

export type BlockNode = Node<BlockData, "block">;

export interface NoteData extends Record<string, unknown> {
  note: Note;
}
export type NoteNode = Node<NoteData, "note">;
export type CanvasNode = BlockNode | NoteNode;

export const NOTE_PREFIX = "note:";
export function isBlock(n: CanvasNode): n is BlockNode {
  return n.type === "block";
}
export function isNote(n: CanvasNode): n is NoteNode {
  return n.type === "note";
}

export function noteNode(note: Note, selected = false): NoteNode {
  return {
    id: NOTE_PREFIX + note.id,
    type: "note",
    position: { ...note.position },
    width: note.width ?? 240,
    height: note.height ?? 120,
    zIndex: -1,
    selected,
    data: { note: clone(note) },
  };
}

export function newNoteId(taken: Iterable<string>): string {
  const ids = new Set(taken);
  let n = 1;
  while (ids.has(`note-${n}`)) n++;
  return `note-${n}`;
}

/** Deep copy that also works on Svelte state proxies (structuredClone doesn't). */
export function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v));
}

export function edgeId(l: Link): string {
  return `${l.from.block}:${l.from.port}->${l.to.block}:${l.to.port}`;
}

export function linkEdge(l: Link): Edge {
  return {
    id: edgeId(l),
    source: l.from.block,
    sourceHandle: l.from.port,
    target: l.to.block,
    targetHandle: l.to.port,
    label: l.label || undefined,
    class: l.from.port === FAILED ? "failed-path" : undefined,
  };
}

export function toCanvas(file: { blocks: Block[]; links: Link[]; notes?: Note[] }): { nodes: CanvasNode[]; edges: Edge[] } {
  const nodes: CanvasNode[] = [
    ...(file.notes ?? []).map((n) => noteNode(n)),
    ...file.blocks.map(
      (b): BlockNode => ({
        id: b.id,
        type: "block",
        position: { ...b.position },
        data: { block: clone(b), inputs: [], outputs: [], issues: [] },
      }),
    ),
  ];
  return { nodes, edges: file.links.map(linkEdge) };
}

/** The blocks, links and notes on the canvas. */
export function toGraph(nodes: CanvasNode[], edges: Edge[]): { blocks: Block[]; links: Link[]; notes: Note[] } {
  const pos = (n: CanvasNode) => ({ x: Math.round(n.position.x), y: Math.round(n.position.y) });
  return {
    blocks: nodes.filter(isBlock).map((n) => ({ ...n.data.block, position: pos(n) })),
    links: edges
      .filter((e) => e.sourceHandle && e.targetHandle)
      .map((e) => {
        const l: Link = {
          from: { block: e.source, port: e.sourceHandle! },
          to: { block: e.target, port: e.targetHandle! },
        };
        if (typeof e.label === "string" && e.label.trim()) l.label = e.label.trim();
        return l;
      }),
    notes: nodes.filter(isNote).map((n) => {
      const note: Note = { ...n.data.note, position: pos(n) };
      const w = n.measured?.width ?? n.width;
      const h = n.measured?.height ?? n.height;
      if (w) note.width = Math.round(w);
      if (h) note.height = Math.round(h);
      return note;
    }),
  };
}

// ------------------------------------------------------------ functions

const key = (b: string, p: string) => `${b}\u0000${p}`;

function freeName(prefix: string, taken: Set<string>): string {
  let n = 1;
  while (taken.has(`${prefix}${n}`)) n++;
  taken.add(`${prefix}${n}`);
  return `${prefix}${n}`;
}

/**
 * Turns the selected blocks into a function. Its inputs are the selected
 * blocks' inputs that no other selected block feeds; its outputs are their
 * outputs that no other selected block uses. Ports linked from outside the
 * selection become ports too, and those links are rewired to the new
 * function block, so the loop behaves the same.
 */
export function makeFunction(
  blocks: Block[],
  links: Link[],
  selected: Set<string>,
  ports: Record<string, BlockPorts>,
  existing: FunctionDef[],
  takenBlockIds: Set<string>,
): { def: FunctionDef; block: Block; blocks: Block[]; links: Link[] } {
  const inner = blocks.filter((b) => selected.has(b.id));
  const innerLinks = links.filter((l) => selected.has(l.from.block) && selected.has(l.to.block));
  const into = links.filter((l) => !selected.has(l.from.block) && selected.has(l.to.block));
  const outOf = links.filter((l) => selected.has(l.from.block) && !selected.has(l.to.block));
  const fedInside = new Set(innerLinks.map((l) => key(l.to.block, l.to.port)));
  const fedOutside = new Set(into.map((l) => key(l.to.block, l.to.port)));
  const usedInside = new Set(innerLinks.map((l) => key(l.from.block, l.from.port)));
  const usedOutside = new Set(outOf.map((l) => key(l.from.block, l.from.port)));

  const names = new Set<string>();
  const inMap = new Map<string, string>();
  const outMap = new Map<string, string>();
  const def: FunctionDef = {
    id: "",
    name: "",
    description: "",
    inputs: [],
    outputs: [],
    blocks: [],
    links: innerLinks,
  };
  for (const b of inner) {
    for (const p of ports[b.id]?.inputs ?? []) {
      const k = key(b.id, p.name);
      if (!fedInside.has(k) || fedOutside.has(k)) {
        const name = freeName("in", names);
        inMap.set(k, name);
        def.inputs.push({ name, label: "", to: { block: b.id, port: p.name } });
      }
    }
    for (const p of ports[b.id]?.outputs ?? []) {
      const k = key(b.id, p.name);
      // An unused Failed output stays closed: the failure stops the run.
      if (p.name === FAILED && !usedOutside.has(k)) continue;
      if (!usedInside.has(k) || usedOutside.has(k)) {
        const name = freeName("out", names);
        outMap.set(k, name);
        def.outputs.push({ name, label: "", from: { block: b.id, port: p.name } });
      }
    }
  }
  const ids = new Set(existing.map((f) => f.id));
  let n = 1;
  while (ids.has(`function-${n}`)) n++;
  def.id = `function-${n}`;
  def.name = `Function ${n}`;

  const xs = inner.map((b) => b.position.x);
  const ys = inner.map((b) => b.position.y);
  const minX = Math.min(...xs);
  const minY = Math.min(...ys);
  def.blocks = inner.map((b) => ({ ...clone(b), position: { x: b.position.x - minX, y: b.position.y - minY } }));

  const blockId = newBlockId("function", takenBlockIds);
  const block: Block = {
    id: blockId,
    type: "function",
    label: def.name,
    position: {
      x: Math.round(xs.reduce((a, b) => a + b, 0) / xs.length),
      y: Math.round(ys.reduce((a, b) => a + b, 0) / ys.length),
    },
    params: { function: def.id },
  };
  const rest = links.filter((l) => !selected.has(l.from.block) && !selected.has(l.to.block));
  const rewired: Link[] = [
    ...into.map((l) => ({ from: l.from, to: { block: blockId, port: inMap.get(key(l.to.block, l.to.port))! } })),
    ...outOf.map((l) => ({ from: { block: blockId, port: outMap.get(key(l.from.block, l.from.port))! }, to: l.to })),
  ];
  const seen = new Set<string>();
  const newLinks = [...rest, ...rewired].filter((l) => {
    const id = edgeId(l);
    if (seen.has(id)) return false;
    seen.add(id);
    return true;
  });
  return { def, block, blocks: [...blocks.filter((b) => !selected.has(b.id)), block], links: newLinks };
}

/**
 * Output ports inside a function that no inner block uses: what it may
 * expose. Failed outputs can be exposed but are never added on their own.
 */
export function outputCandidates(def: FunctionDef, ports: Record<string, BlockPorts>): { block: string; port: PortSpec }[] {
  const used = new Set(def.links.map((l) => key(l.from.block, l.from.port)));
  return def.blocks.flatMap((b) =>
    (ports[b.id]?.outputs ?? []).filter((p) => !used.has(key(b.id, p.name))).map((port) => ({ block: b.id, port })),
  );
}

/**
 * Keeps a function's ports matching its blocks after an edit: every input no
 * inner block feeds becomes an input (or it could never run), and inputs and
 * outputs whose port is gone are dropped. Newly open outputs not seen
 * before are added.
 * Returns the updated function, or null if nothing changed.
 */
export function syncFunctionPorts(
  def: FunctionDef,
  ports: Record<string, BlockPorts>,
  knownOutputs: Set<string>,
): FunctionDef | null {
  const fed = new Set(def.links.map((l) => key(l.to.block, l.to.port)));
  const names = new Set([...def.inputs.map((i) => i.name), ...def.outputs.map((o) => o.name)]);
  const wantIn = def.blocks.flatMap((b) =>
    (ports[b.id]?.inputs ?? []).filter((p) => !fed.has(key(b.id, p.name))).map((p) => key(b.id, p.name)),
  );
  // Keep inputs whose port still exists, even if a block inside also feeds
  // it (a cycle back into the first block is common), and add any port
  // nothing feeds.
  const hasIn = (b: string, p: string) => (ports[b]?.inputs ?? []).some((x) => x.name === p);
  const inputs = def.inputs.filter((i) => hasIn(i.to.block, i.to.port));
  for (const k of wantIn) {
    if (!inputs.some((i) => key(i.to.block, i.to.port) === k)) {
      const [block, port] = k.split("\u0000");
      inputs.push({ name: freeName("in", names), label: "", to: { block, port } });
    }
  }
  const exists = (b: string, p: string) => (ports[b]?.outputs ?? []).some((x) => x.name === p);
  const outputs = def.outputs.filter((o) => exists(o.from.block, o.from.port));
  for (const c of outputCandidates(def, ports)) {
    const k = key(c.block, c.port.name);
    if (c.port.name === FAILED) continue;
    if (!knownOutputs.has(k)) {
      knownOutputs.add(k);
      if (!outputs.some((o) => key(o.from.block, o.from.port) === k)) {
        outputs.push({ name: freeName("out", names), label: "", from: { block: c.block, port: c.port.name } });
      }
    }
  }
  const changed = JSON.stringify([inputs, outputs]) !== JSON.stringify([def.inputs, def.outputs]);
  return changed ? { ...def, inputs, outputs } : null;
}

export function portKey(block: string, port: string): string {
  return key(block, port);
}

/** A key that changes only when the loop's structure (not positions) changes. */
export function structureKey(meta: LoopFile, nodes: CanvasNode[], edges: Edge[]): string {
  return JSON.stringify([
    meta.limits,
    meta.functions ?? [],
    nodes.filter(isBlock).map((n) => [n.id, n.data.block.type, n.data.block.label, n.data.block.params]),
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

export { categoryColor, categoryLabel } from "$lib/blocks";

/** One-line summary shown on a block. */
export function summary(b: Block): string {
  const p = b.params as Record<string, unknown>;
  const s = (v: unknown) => (typeof v === "string" ? v : "");
  switch (b.type) {
    case "llm": {
      const tools = p.tools === "off" ? "" : describeTools(p.tools);
      const prompt = s(p.prompt).replace(/\s+/g, " ").trim();
      const sys = s(p.system).replace(/\s+/g, " ").trim();
      const base = prompt && prompt !== "_INPUT_" ? prompt : sys;
      const mode = p.tool_mode === "prompted" ? " (prompted)" : p.tool_mode === "off" ? " (off)" : "";
      return tools ? `${tools}${mode} · ${base}` : base;
    }
    case "augment":
      return `${s(p.role) || "user"}: ${s(p.format).replace(/\s+/g, " ").trim()}`;
    case "exchange":
      return s(p.system).replace(/\s+/g, " ").trim() || (Array.isArray(p.roles) ? p.roles.join(", ") : "");
    case "read_exchange":
      return `Last from ${s(p.role) || "assistant"}`;
    case "for_each":
      return p.limit ? `At most ${p.limit}` : "";
    case "function":
      return "Double-click to open";
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
    case "ask_user": {
      const q = s(p.prompt).replace(/\s+/g, " ").trim();
      const t = Number(p.timeout_minutes) || 0;
      return [q && q !== "_INPUT_" ? q : "Asks you the input", t ? `gives up after ${t} min` : ""].filter(Boolean).join(" · ");
    }
    case "run_tool":
      return p.confirm ? "Asks you first" : "";
    case "store":
      return `Keeps {{${s(p.key)}}}`;
    case "recall":
      return `Reads {{${s(p.key)}}}`;
    case "outline":
      return p.action === "add_steps"
        ? `Adds steps to {{${s(p.key) || "plan"}}}`
        : p.action === "mark"
          ? `Marks a task ${s(p.status) || "done"}`
          : `Starts {{${s(p.key) || "plan"}}}`;
    case "find_call": {
      return describeTools(p.tools);
    }
    case "decline_calls":
      return "Answers each call: not run";
    case "stop":
      return "Ends the run as failed";
    default:
      return "";
  }
}

export function blankLoop(name = "New loop"): LoopFile {
  return {
    format: "chummy.loop",
    version: 3,
    name,
    description: "",
    limits: { max_steps: 200, max_llm_calls: 20, max_tool_calls: 20 },
    blocks: [],
    links: [],
  };
}

/** Set by the editor so blocks can ask it to do things. */
export const editorActions: {
  openFunction: (functionId: string) => void;
  /** A note's text changed on the canvas. */
  setNote: (nodeId: string, text: string) => void;
} = {
  openFunction: () => {},
  setNote: () => {},
};
