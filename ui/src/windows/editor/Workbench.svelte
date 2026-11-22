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

  import { commands, events, errorText, type Analysis, type Block, type BlockInfo, type LoopFile, type Template } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import BlockNodeView from "./BlockNode.svelte";
  import Palette from "./Palette.svelte";
  import Inspector from "./Inspector.svelte";
  import RunPanel from "./RunPanel.svelte";
  import StartScreen from "./StartScreen.svelte";
  import { blankLoop, clone, newBlock, newBlockId, structureKey, toCanvas, toFile, type BlockNode } from "./loopdoc";

  const nodeTypes = { block: BlockNodeView };
  const flow = useSvelteFlow();

  let view = $state<"start" | "edit">("start");
  let loopId = $state<string | null>(null);
  let meta = $state<LoopFile>(blankLoop());
  let nodes = $state.raw<BlockNode[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let analysis = $state<Analysis>({ ports: {}, issues: [], ok: true });
  let selected = $state<string | null>(null);
  let savedKey = $state("");
  let saving = $state(false);
  let notice = $state("");
  let showRun = $state(false);
  let runId = $state<string | null>(null);
  let pending = $state<(() => void) | null>(null);
  let canvasEl = $state<HTMLDivElement>();

  const file = () => toFile(meta, nodes, edges);
  const currentKey = $derived(structureKey(meta, nodes, edges) + JSON.stringify([meta.name, meta.description, nodes.map((n) => [Math.round(n.position.x), Math.round(n.position.y)])]));
  const dirty = $derived(view === "edit" && currentKey !== savedKey);
  const errors = $derived(analysis.issues.filter((i) => i.severity === "error").length);
  const warnings = $derived(analysis.issues.filter((i) => i.severity === "warning").length);
  const selectedBlock = $derived(nodes.find((n) => n.id === selected)?.data.block ?? null);

  onMount(() => {
    loops.init();
    runs.init();
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
    meta = { ...f, blocks: [], links: [] };
    nodes = c.nodes;
    edges = c.edges;
    loopId = id;
    selected = null;
    runId = null;
    lastAnalyzed = "";
    view = "edit";
    savedKey = id ? structureKey(meta, nodes, edges) + JSON.stringify([meta.name, meta.description, nodes.map((n) => [Math.round(n.position.x), Math.round(n.position.y)])]) : "";
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
    const key = structureKey(meta, nodes, edges);
    if (key === lastAnalyzed) return;
    clearTimeout(timer);
    timer = setTimeout(async () => {
      lastAnalyzed = key;
      const a = await commands.analyzeLoop(untrack(file));
      analysis = a;
      untrack(() => applyAnalysis(a));
      if (fitPending) {
        fitPending = false;
        setTimeout(() => flow.fitView({ padding: 0.2, maxZoom: 1.1 }), 80);
      }
    }, 120);
  });

  function applyAnalysis(a: Analysis) {
    nodes = nodes.map((n) => {
      const p = a.ports[n.id] ?? { inputs: [], outputs: [] };
      const issues = a.issues.filter((i) => i.block === n.id && i.severity === "error").map((i) => i.message);
      return { ...n, data: { ...n.data, inputs: p.inputs, outputs: p.outputs, issues } };
    });
    // Drop links whose ports no longer exist (e.g. fewer Combine inputs).
    const ok = (e: Edge) =>
      a.ports[e.source]?.outputs.some((p) => p.name === e.sourceHandle) &&
      a.ports[e.target]?.inputs.some((p) => p.name === e.targetHandle);
    if (edges.some((e) => a.ports[e.source] && a.ports[e.target] && !ok(e))) {
      edges = edges.filter((e) => !(a.ports[e.source] && a.ports[e.target]) || ok(e));
    }
  }

  // ------------------------------------------------------------ editing

  function addBlock(info: BlockInfo, at?: { x: number; y: number }) {
    const taken = new Set(nodes.map((n) => n.id));
    const id = newBlockId(info.kind, taken);
    let pos = at;
    if (!pos) {
      const r = canvasEl!.getBoundingClientRect();
      pos = flow.screenToFlowPosition({ x: r.left + r.width / 2 - 110, y: r.top + r.height / 2 - 40 });
      pos.x += (nodes.length % 5) * 24;
      pos.y += (nodes.length % 5) * 24;
    }
    const block = newBlock(info, id, pos.x, pos.y);
    nodes = [
      ...nodes.map((n) => ({ ...n, selected: false })),
      { id, type: "block", position: { ...block.position }, selected: true, data: { block, inputs: [], outputs: [], issues: [] } },
    ];
    selected = id;
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    const kind = e.dataTransfer?.getData("application/x-chummy-block");
    const info = kind ? loops.info(kind) : undefined;
    if (info) addBlock(info, flow.screenToFlowPosition({ x: e.clientX - 110, y: e.clientY - 20 }));
  }

  function updateBlock(b: Block) {
    nodes = nodes.map((n) => (n.id === b.id ? { ...n, data: { ...n.data, block: b } } : n));
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
    const out = analysis.ports[c.source]?.outputs.find((p) => p.name === c.sourceHandle);
    const inp = analysis.ports[c.target]?.inputs.find((p) => p.name === c.targetHandle);
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
      if (!nodes.some((n) => n.data.state !== states[n.id] || n.data.output !== outputs[n.id])) return;
      nodes = nodes.map((n) => ({ ...n, data: { ...n.data, state: states[n.id], output: outputs[n.id] } }));
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
      <Button variant="ghost" icon="chevron-left" onclick={() => guard(() => ((view = "start"), (runId = null), history.replaceState(null, "", "#")))}>Loops</Button>
      <input class="name" bind:value={meta.name} aria-label="Loop name" spellcheck="false" />
      <span class="state" class:dirty>{dirty ? "Unsaved" : loopId ? "Saved" : ""}</span>
      {#if errors}<span class="badge danger">{errors} error{errors === 1 ? "" : "s"}</span>{/if}
      {#if warnings}<span class="badge warn">{warnings} warning{warnings === 1 ? "" : "s"}</span>{/if}
      <div class="spacer"></div>
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
      <Palette onadd={(b) => addBlock(b)} />
      <div class="canvas" bind:this={canvasEl} ondragover={(e) => { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = "copy"; }} ondrop={onDrop} role="application">
        <SvelteFlow
          bind:nodes
          bind:edges
          {nodeTypes}
          {isValidConnection}
          forceColorMode={settings.resolvedTheme}
          deleteKey={["Delete", "Backspace"]}
          defaultEdgeOptions={{ type: "default" }}
          onselectionchange={({ nodes: sel }) => (selected = sel.length === 1 ? sel[0].id : null)}
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
        <Inspector block={selectedBlock} bind:meta issues={analysis.issues} onchange={updateBlock} ondelete={deleteBlock} onselect={selectBlock} />
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
