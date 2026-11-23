<script lang="ts">
  import { onMount, untrack } from "svelte";
  import {
    SvelteFlow,
    Background,
    BackgroundVariant,
    Controls,
    MiniMap,
    useSvelteFlow,
    type Connection,
    type Edge,
  } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";

  import { commands, events, errorText, type Analysis, type Block, type BlockInfo, type FunctionDef, type Link, type LoopFile, type Note, type Template } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import BlockNodeView from "./BlockNode.svelte";
  import NoteNodeView from "./NoteNode.svelte";
  import Palette from "./Palette.svelte";
  import Inspector from "./Inspector.svelte";
  import RunPanel from "./RunPanel.svelte";
  import StartScreen from "./StartScreen.svelte";
  import {
    NOTE_PREFIX,
    blankLoop,
    clone,
    editorActions,
    FAILED,
    isBlock,
    newNoteId,
    noteNode,
    makeFunction,
    newBlock,
    newBlockId,
    outputCandidates,
    portKey,
    syncFunctionPorts,
    toCanvas,
    toGraph,
    type CanvasNode,
  } from "./loopdoc";

  const nodeTypes = { block: BlockNodeView, note: NoteNodeView };
  const flow = useSvelteFlow();

  let view = $state<"start" | "edit">("start");
  let loopId = $state<string | null>(null);
  let meta = $state<LoopFile>(blankLoop());
  let nodes = $state.raw<CanvasNode[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let analysis = $state<Analysis>({ ports: {}, functions: {}, issues: [], ok: true });
  let selected = $state<string | null>(null);
  /** The selected link, when only a link is selected. */
  let selectedEdge = $state<string | null>(null);
  /** Every selected block (for Make function). */
  let selection = $state<string[]>([]);
  /** The function being edited; null means the loop itself. */
  let scope = $state<string | null>(null);
  /** The loop's own blocks and links while a function is open. */
  let stash = $state.raw<{ blocks: Block[]; links: Link[]; notes: Note[] }>({ blocks: [], links: [], notes: [] });
  /** Per function: open outputs already offered, so unticked ones stay unticked. */
  const known = new Map<string, Set<string>>();
  let savedKey = $state("");
  let saving = $state(false);
  let notice = $state("");
  let showRun = $state(false);
  let runId = $state<string | null>(null);
  let pending = $state<(() => void) | null>(null);
  let canvasEl = $state<HTMLDivElement>();

  /** The whole loop file: the open graph merged into the rest. */
  function file(): LoopFile {
    const cur = toGraph(nodes, edges);
    const fns = meta.functions ?? [];
    if (scope === null) return { ...meta, ...cur, functions: fns };
    return { ...meta, ...stash, functions: fns.map((f) => (f.id === scope ? { ...f, ...cur } : f)) };
  }
  /** Notes, comments and labels don't change what the loop does. */
  function forAnalysis(f: LoopFile): LoopFile {
    const bare = (bs: Block[]) => bs.map(({ comment: _c, ...b }) => b);
    const links = (ls: Link[]) => ls.map(({ label: _l, ...l }) => l);
    return {
      ...f,
      notes: [],
      blocks: bare(f.blocks),
      links: links(f.links),
      functions: (f.functions ?? []).map((x) => ({ ...x, notes: [], blocks: bare(x.blocks), links: links(x.links) })),
    };
  }
  /** The file without positions: changes only when its meaning changes. */
  function structure(f: LoopFile): string {
    const g = forAnalysis(f);
    const strip = (bs: Block[]) => bs.map((b) => ({ ...b, position: null }));
    return JSON.stringify({ ...g, name: "", description: "", blocks: strip(g.blocks), functions: (g.functions ?? []).map((x) => ({ ...x, blocks: strip(x.blocks) })) });
  }
  /**
   * A form of the file that only changes when its content does: keys sorted,
   * and empty strings, lists and unset values left out (opening a function
   * fills some of these in without changing anything).
   */
  function canon(v: unknown): unknown {
    if (Array.isArray(v)) return v.map(canon);
    if (v && typeof v === "object") {
      const out: Record<string, unknown> = {};
      for (const k of Object.keys(v).sort()) {
        const x = (v as Record<string, unknown>)[k];
        if (x === undefined || x === null || x === "" || (Array.isArray(x) && !x.length)) continue;
        out[k] = canon(x);
      }
      return out;
    }
    return v;
  }
  const fileKey = (f: LoopFile) => JSON.stringify(canon(f));
  const currentKey = $derived(fileKey(file()));
  const scopeDef = $derived(scope ? (meta.functions ?? []).find((f) => f.id === scope) ?? null : null);
  const scopePorts = $derived(scope ? analysis.functions[scope]?.blocks ?? {} : analysis.ports);
  const scopeIssues = $derived(analysis.issues.filter((i) => (i.function ?? null) === scope));
  const uses = $derived.by(() => {
    if (!scope) return 0;
    const f = file();
    return [...f.blocks, ...(f.functions ?? []).flatMap((x) => x.blocks)].filter((b) => b.type === "function" && b.params.function === scope).length;
  });
  const dirty = $derived(view === "edit" && currentKey !== savedKey);
  const errors = $derived(analysis.issues.filter((i) => i.severity === "error").length);
  editorActions.openFunction = (id) => enterFunction(id);
  editorActions.setNote = (id, text) => {
    nodes = nodes.map((n) => (n.id === id && n.type === "note" ? { ...n, data: { note: { ...n.data.note, text } } } : n));
  };
  const warnings = $derived(analysis.issues.filter((i) => i.severity === "warning").length);
  const selectedBlock = $derived.by(() => {
    const n = nodes.find((x) => x.id === selected);
    return n && isBlock(n) ? n.data.block : null;
  });
  const selectedLink = $derived(edges.find((e) => e.id === selectedEdge) ?? null);

  onMount(() => {
    loops.init();
    runs.init();
    asks.init();
    const go = (section: string) => {
      if (section === "import") guard(importLoop);
      else if (section) guard(() => openLoop(section));
    };
    go(location.hash.replace(/^#/, ""));
    events.onGoto(go);
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s" && view === "edit") {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // ------------------------------------------------------------ loading

  function load(f: LoopFile, id: string | null) {
    const c = toCanvas(f);
    meta = { ...f, blocks: [], links: [], notes: [], functions: f.functions ?? [] };
    scope = null;
    stash = { blocks: [], links: [], notes: [] };
    known.clear();
    nodes = c.nodes;
    edges = c.edges;
    loopId = id;
    selected = null;
    selectedEdge = null;
    runId = null;
    lastAnalyzed = "";
    view = "edit";
    savedKey = id ? fileKey(file()) : "";
    fitPending = true;
  }
  /** Fit the view once blocks have their real sizes (after the first analysis). */
  let fitPending = false;

  async function openLoop(id: string) {
    try {
      load(await commands.loadLoop(id), id);
    } catch (e) {
      notice = errorText(e);
    }
  }

  function fromTemplate(t: Template) {
    load(clone(t.file), null);
  }

  async function importLoop() {
    const id = await commands.importLoop();
    if (id) await openLoop(id);
  }

  /** Runs `then` now, or after asking about unsaved changes. */
  function guard(then: () => void) {
    if (dirty) pending = then;
    else then();
  }

  // ------------------------------------------------------------ saving

  async function save() {
    saving = true;
    notice = "";
    try {
      const keyAtSave = currentKey;
      loopId = await commands.saveLoop(loopId, file());
      savedKey = keyAtSave;
      history.replaceState(null, "", `#${loopId}`);
    } catch (e) {
      notice = errorText(e);
    } finally {
      saving = false;
    }
  }

  // ------------------------------------------------------------ analysis

  let lastAnalyzed = "";
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (view !== "edit") return;
    const key = structure(file());
    if (key === lastAnalyzed) return;
    clearTimeout(timer);
    timer = setTimeout(async () => {
      lastAnalyzed = key;
      const a = await commands.analyzeLoop(forAnalysis(untrack(file)));
      analysis = a;
      untrack(() => applyAnalysis(a));
      if (fitPending) {
        fitPending = false;
        setTimeout(() => flow.fitView({ padding: 0.2, maxZoom: 1.1 }), 80);
      }
    }, 120);
  });

  function knownFor(id: string): Set<string> {
    let k = known.get(id);
    if (!k) known.set(id, (k = new Set()));
    return k;
  }

  function applyAnalysis(a: Analysis) {
    const ports = scope ? a.functions[scope]?.blocks : a.ports;
    if (!ports) return;
    let def = scope ? (meta.functions ?? []).find((f) => f.id === scope) : undefined;
    if (def) {
      // Keep the function's inputs and outputs matching its blocks.
      const synced = syncFunctionPorts({ ...def, ...toGraph(nodes, edges) }, ports, knownFor(def.id));
      if (synced) {
        def = { ...def, inputs: synced.inputs, outputs: synced.outputs };
        meta.functions = (meta.functions ?? []).map((f) => (f.id === scope ? def! : f));
      }
    }
    nodes = nodes.map((n) => {
      if (!isBlock(n)) return n;
      const p = ports[n.id] ?? { inputs: [], outputs: [] };
      const issues = a.issues
        .filter((i) => i.block === n.id && (i.function ?? null) === scope && i.severity === "error")
        .map((i) => i.message);
      const exposedIn = def?.inputs.filter((x) => x.to.block === n.id).map((x) => x.to.port);
      const exposedOut = def?.outputs.filter((x) => x.from.block === n.id).map((x) => x.from.port);
      return { ...n, data: { ...n.data, inputs: p.inputs, outputs: p.outputs, issues, exposedIn, exposedOut } };
    });
    // Drop links whose ports no longer exist (e.g. fewer Combine inputs).
    const ok = (e: Edge) =>
      ports[e.source]?.outputs.some((p) => p.name === e.sourceHandle) &&
      ports[e.target]?.inputs.some((p) => p.name === e.targetHandle);
    if (edges.some((e) => ports[e.source] && ports[e.target] && !ok(e))) {
      edges = edges.filter((e) => !(ports[e.source] && ports[e.target]) || ok(e));
    }
    // Paths that carry failures look different.
    const cls = (e: Edge) => (e.sourceHandle === FAILED ? "failed-path" : undefined);
    if (edges.some((e) => e.class !== cls(e))) edges = edges.map((e) => ({ ...e, class: cls(e) }));
  }

  // ------------------------------------------------------------ functions

  function show(blocks: Block[], links: Link[], notes: Note[] = [], select: string | null = null) {
    const c = toCanvas({ blocks, links, notes });
    nodes = c.nodes.map((n) => ({ ...n, selected: n.id === select }));
    edges = c.edges;
    selected = select;
    selection = select ? [select] : [];
    selectedEdge = null;
    lastAnalyzed = "";
    fitPending = true;
  }

  function enterFunction(id: string, select: string | null = null) {
    const f = file();
    const def = (f.functions ?? []).find((x) => x.id === id);
    if (!def) return;
    if (scope === null) stash = { blocks: f.blocks, links: f.links, notes: f.notes ?? [] };
    meta = { ...meta, functions: f.functions ?? [] };
    if (!known.has(id)) {
      const ports = analysis.functions[id]?.blocks ?? {};
      known.set(id, new Set(outputCandidates(def, ports).map((c) => portKey(c.block, c.port.name))));
    }
    scope = id;
    show(def.blocks, def.links, def.notes ?? [], select);
  }

  function exitFunction(select: string | null = null) {
    if (scope === null) return;
    const f = file();
    meta = { ...meta, functions: f.functions ?? [] };
    scope = null;
    show(stash.blocks, stash.links, stash.notes, select);
  }

  function makeFn() {
    const g = toGraph(nodes, edges);
    const picked = new Set(selection.filter((id) => !id.startsWith(NOTE_PREFIX)));
    if (!picked.size) return;
    const r = makeFunction(g.blocks, g.links, picked, scopePorts, meta.functions ?? [], new Set(g.blocks.map((b) => b.id)));
    known.set(r.def.id, new Set(outputCandidates(r.def, scopePorts).map((c) => portKey(c.block, c.port.name))));
    meta = { ...meta, functions: [...(meta.functions ?? []), r.def] };
    show(r.blocks, r.links, g.notes, r.block.id);
    fitPending = false;
  }

  function addFunctionBlock(def: FunctionDef) {
    const info = loops.info("function");
    if (!info) return;
    addBlock({ ...info, label: def.name, default_params: { function: def.id } });
  }

  function updateFunction(def: FunctionDef) {
    meta.functions = (meta.functions ?? []).map((f) => (f.id === def.id ? { ...def, blocks: f.blocks, links: f.links } : f));
    applyAnalysis(analysis);
  }

  function deleteFunction(id: string) {
    if (scope === id) exitFunction();
    meta = { ...meta, functions: (meta.functions ?? []).filter((f) => f.id !== id) };
  }

  function jumpTo(id: string, fn: string | null) {
    if (fn !== scope) {
      if (fn) enterFunction(fn, id);
      else exitFunction(id);
      return;
    }
    selectBlock(id);
  }

  // ------------------------------------------------------------ editing

  function centre(): { x: number; y: number } {
    const r = canvasEl!.getBoundingClientRect();
    const pos = flow.screenToFlowPosition({ x: r.left + r.width / 2 - 110, y: r.top + r.height / 2 - 40 });
    pos.x += (nodes.length % 5) * 24;
    pos.y += (nodes.length % 5) * 24;
    return pos;
  }

  function addBlock(info: BlockInfo, at?: { x: number; y: number }) {
    const taken = new Set(nodes.map((n) => n.id));
    const id = newBlockId(info.kind, taken);
    const pos = at ?? centre();
    const block = newBlock(info, id, pos.x, pos.y);
    nodes = [
      ...nodes.map((n) => ({ ...n, selected: false })),
      { id, type: "block", position: { ...block.position }, selected: true, data: { block, inputs: [], outputs: [], issues: [] } },
    ];
    selected = id;
  }

  function addNote() {
    const id = newNoteId(nodes.filter((n) => n.type === "note").map((n) => n.id.slice(NOTE_PREFIX.length)));
    const p = centre();
    const note: Note = { id, text: "", position: { x: Math.round(p.x), y: Math.round(p.y) }, width: 240, height: 120 };
    nodes = [...nodes.map((n) => ({ ...n, selected: false })), noteNode(note, true)];
    selected = null;
  }

  function setLinkLabel(id: string, label: string) {
    edges = edges.map((e) => (e.id === id ? { ...e, label: label || undefined } : e));
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    const kind = e.dataTransfer?.getData("application/x-chummy-block");
    const info = kind ? loops.info(kind) : undefined;
    if (info) addBlock(info, flow.screenToFlowPosition({ x: e.clientX - 110, y: e.clientY - 20 }));
  }

  function updateBlock(b: Block) {
    nodes = nodes.map((n) => (n.id === b.id && isBlock(n) ? { ...n, data: { ...n.data, block: b } } : n));
  }

  function deleteBlock(id: string) {
    nodes = nodes.filter((n) => n.id !== id);
    edges = edges.filter((e) => e.source !== id && e.target !== id);
    if (selected === id) selected = null;
  }

  function selectBlock(id: string) {
    selected = id;
    nodes = nodes.map((n) => ({ ...n, selected: n.id === id }));
    const n = nodes.find((x) => x.id === id);
    if (n) flow.setCenter(n.position.x + 110, n.position.y + 60, { duration: 300, zoom: flow.getZoom() });
  }

  function isValidConnection(c: Edge | Connection): boolean {
    const out = scopePorts[c.source]?.outputs.find((p) => p.name === c.sourceHandle);
    const inp = scopePorts[c.target]?.inputs.find((p) => p.name === c.targetHandle);
    if (!out || !inp) return false;
    const fits = out.kind === inp.kind || out.kind === "any" || inp.kind === "any";
    const dup = edges.some(
      (e) => e.source === c.source && e.target === c.target && e.sourceHandle === c.sourceHandle && e.targetHandle === c.targetHandle,
    );
    return fits && !dup;
  }

  // ------------------------------------------------------------ test runs

  // Show live block states from the current test run on the canvas.
  $effect(() => {
    const r = runId ? runs.runs[runId] : undefined;
    const states = r?.blocks ?? {};
    const outputs = r?.outputs ?? {};
    void JSON.stringify(states);
    void JSON.stringify(outputs);
    untrack(() => {
      // Inside a function, blocks are named "<function block>/<block>". Only
      // count blocks whose parent is a block of the function that's open.
      let callers = new Set<string>();
      if (scope !== null) {
        const f = file();
        for (const b of [...f.blocks, ...(f.functions ?? []).flatMap((x) => x.blocks)]) {
          if (b.type === "function" && b.params.function === scope) callers.add(b.id);
        }
      }
      const pick = <T,>(m: Record<string, T>, id: string): T | undefined => {
        if (scope === null) return m[id];
        let found: T | undefined;
        for (const [k, v] of Object.entries(m)) {
          const parts = k.split("/");
          if (parts.length > 1 && parts[parts.length - 1] === id && callers.has(parts[parts.length - 2])) {
            found = v;
            if (v === "running") break;
          }
        }
        return found;
      };
      const next = nodes.map((n) => ({ id: n.id, state: pick(states, n.id), output: pick(outputs, n.id) }));
      if (!nodes.some((n, i) => isBlock(n) && (n.data.state !== next[i].state || n.data.output !== next[i].output))) return;
      nodes = nodes.map((n, i) => (isBlock(n) ? { ...n, data: { ...n.data, state: next[i].state, output: next[i].output } } : n));
    });
  });

  function closeRun() {
    showRun = false;
    runId = null;
  }
</script>

<div class="editor">
  <header class="toolbar">
    {#if view === "edit"}
      {#if scopeDef}
        <Button variant="ghost" icon="chevron-left" onclick={() => exitFunction()}>{meta.name}</Button>
        <span class="crumb"><Icon name="layers" size={14} /> {scopeDef.name}</span>
      {:else}
        <Button variant="ghost" icon="chevron-left" onclick={() => guard(() => ((view = "start"), (runId = null), history.replaceState(null, "", "#")))}>Loops</Button>
        <input class="name" bind:value={meta.name} aria-label="Loop name" spellcheck="false" />
      {/if}
      <span class="state" class:dirty>{dirty ? "Unsaved" : loopId ? "Saved" : ""}</span>
      {#if errors}<span class="badge danger">{errors} error{errors === 1 ? "" : "s"}</span>{/if}
      {#if warnings}<span class="badge warn">{warnings} warning{warnings === 1 ? "" : "s"}</span>{/if}
      <div class="spacer"></div>
      {#if selection.length > 0}
        <Button variant="ghost" icon="layers" onclick={makeFn} title="Group the selected blocks into one block">Make function</Button>
      {/if}
      {#if loopId}<Button variant="ghost" icon="export" onclick={() => commands.exportLoop(loopId!)}>Export</Button>{/if}
      <Button icon="play" onclick={() => (showRun = !showRun)}>{showRun ? "Hide run" : "Test run"}</Button>
      <Button variant="primary" onclick={save} disabled={saving || (!dirty && !!loopId)}>{loopId ? "Save" : "Save loop"}</Button>
    {:else}
      <span class="brand">Loop editor</span>
      <div class="spacer"></div>
    {/if}
  </header>
  {#if notice}<div class="notice-bar" role="alert"><Icon name="alert" size={15} /><span>{notice}</span><button type="button" onclick={() => (notice = "")}>Dismiss</button></div>{/if}

  {#if view === "start"}
    <StartScreen onopen={(id) => openLoop(id)} ontemplate={fromTemplate} onimport={importLoop} />
  {:else}
    <div class="body">
      <Palette onadd={(b) => addBlock(b)} onaddnote={addNote} functions={meta.functions ?? []} current={scope} onaddfunction={addFunctionBlock} />
      <div class="canvas" bind:this={canvasEl} ondragover={(e) => { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = "copy"; }} ondrop={onDrop} role="application">
        <SvelteFlow
          bind:nodes
          bind:edges
          {nodeTypes}
          {isValidConnection}
          forceColorMode={settings.resolvedTheme}
          deleteKey={["Delete", "Backspace"]}
          defaultEdgeOptions={{ type: "default" }}
          onselectionchange={({ nodes: sel, edges: selEdges }) => {
            const blocks = sel.filter((n) => n.type === "block");
            selected = blocks.length === 1 && sel.length === 1 ? blocks[0].id : null;
            selection = blocks.map((n) => n.id);
            selectedEdge = sel.length === 0 && selEdges.length === 1 ? selEdges[0].id : null;
          }}
          minZoom={0.2}
          maxZoom={2}
        >
          <Background variant={BackgroundVariant.Dots} gap={22} size={1.4} />
          <Controls showLock={false} />
          {#if nodes.length > 6}<MiniMap pannable zoomable />{/if}
        </SvelteFlow>
        {#if nodes.length === 0}
          <div class="hint" aria-hidden="true">
            <p class="hint-title">This loop is empty</p>
            <p>Add blocks from the left, then drag from an output dot to an input dot to link them.</p>
          </div>
        {/if}
      </div>
      {#if showRun}
        <RunPanel {file} canRun={analysis.ok} bind:runId onclose={closeRun} />
      {:else}
        <Inspector
          block={selectedBlock}
          link={selectedLink}
          onlinklabel={setLinkLabel}
          bind:meta
          issues={selectedBlock ? scopeIssues : scope ? scopeIssues : analysis.issues}
          onchange={updateBlock}
          ondelete={deleteBlock}
          onselect={jumpTo}
          scope={scopeDef}
          innerPorts={scopePorts}
          {uses}
          onfunctionchange={updateFunction}
          onopenfunction={(id) => enterFunction(id)}
          ondeletefunction={deleteFunction}
        />
      {/if}
    </div>
  {/if}
</div>

{#if pending}
  <Modal title="Save changes to “{meta.name}”?" onclose={() => (pending = null)}>
    Your changes will be lost if you don't save them.
    {#snippet actions()}
      <Button variant="ghost" onclick={() => (pending = null)}>Cancel</Button>
      <Button onclick={() => { const p = pending!; pending = null; savedKey = currentKey; p(); }}>Don't save</Button>
      <Button variant="primary" onclick={async () => { const p = pending!; pending = null; await save(); if (!dirty) p(); }}>Save</Button>
    {/snippet}
  </Modal>
{/if}

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .toolbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: var(--topbar-h);
    padding: 0 var(--space-3);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .brand {
    padding: 0 var(--space-2);
    font-weight: 650;
  }
  .name {
    width: 240px;
    height: 30px;
    padding: 0 var(--space-2);
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    font-weight: 600;
    outline: none;
  }
  .name:hover {
    border-color: var(--border);
  }
  .name:focus {
    border-color: var(--border-strong);
    background: var(--surface-2);
  }
  .crumb {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .state {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .state.dirty {
    color: var(--warn);
  }
  .badge {
    padding: 1px var(--space-2);
    border-radius: var(--radius-full);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .badge.danger {
    background: color-mix(in srgb, var(--danger) 14%, transparent);
    color: var(--danger);
  }
  .badge.warn {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .spacer {
    flex: 1;
  }
  .notice-bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    font-size: var(--text-sm);
  }
  .notice-bar span {
    flex: 1;
  }
  .notice-bar button {
    border: none;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .canvas {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  .canvas :global(.svelte-flow) {
    --xy-background-color: var(--bg);
    --xy-background-pattern-color: color-mix(in srgb, var(--text-faint) 45%, transparent);
    --xy-minimap-background-color: var(--surface);
    --xy-controls-button-background-color: var(--surface);
    --xy-controls-button-background-color-hover: var(--surface-hover);
    --xy-controls-button-color: var(--text-muted);
    --xy-controls-button-border-color: var(--border);
    --xy-attribution-background-color: transparent;
    --xy-edge-stroke: var(--text-faint);
    --xy-edge-stroke-selected: var(--accent);
    --xy-edge-stroke-width: 1.6;
    --xy-connectionline-stroke: var(--accent);
    --xy-node-border-radius: var(--radius-lg);
  }
  .canvas :global(.svelte-flow__edge.failed-path .svelte-flow__edge-path) {
    stroke: color-mix(in srgb, var(--danger) 70%, var(--text-faint));
    stroke-dasharray: 5 4;
  }
  .canvas :global(.svelte-flow__edge-label) {
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text-muted);
    font-size: 11px;
  }
  .canvas :global(.svelte-flow__node-note) {
    padding: 0;
    border: none;
    background: transparent;
    box-shadow: none;
  }
  .canvas :global(.svelte-flow__node-block) {
    padding: 0;
    border: none;
    background: transparent;
    box-shadow: none;
  }
  .canvas :global(.svelte-flow__controls),
  .canvas :global(.svelte-flow__minimap) {
    border-radius: var(--radius-md);
    overflow: hidden;
    border: 1px solid var(--border);
    box-shadow: var(--shadow-sm);
  }
  .hint {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    text-align: center;
    pointer-events: none;
    color: var(--text-faint);
    font-size: var(--text-sm);
  }
  .hint-title {
    color: var(--text-muted);
    font-size: var(--text-lg);
    font-weight: 600;
    margin-bottom: var(--space-1);
  }
</style>
