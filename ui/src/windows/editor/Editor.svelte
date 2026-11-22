<script lang="ts">
  import {
    SvelteFlow,
    Background,
    BackgroundVariant,
    Controls,
    MiniMap,
    type Node,
    type Edge,
  } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";

  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";

  // A new loop starts empty: nothing is installed by default.
  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let name = $state("Untitled loop");
</script>

<div class="editor">
  <header class="toolbar">
    <input class="name" bind:value={name} aria-label="Loop name" spellcheck="false" />
    <div class="spacer"></div>
    <Button variant="ghost" icon="import" disabled>Import</Button>
    <Button variant="ghost" disabled>Export</Button>
    <Button variant="primary" icon="play" disabled>Run</Button>
  </header>

  <div class="body">
    <aside class="palette" aria-label="Blocks">
      <div class="palette-title">Blocks</div>
      <EmptyState icon="flow" title="No blocks installed" compact>
        Input, LLM call, tool call, verify, branch and more will live here.
      </EmptyState>
    </aside>

    <div class="canvas">
      <SvelteFlow bind:nodes bind:edges forceColorMode={settings.resolvedTheme} fitView>
        <Background variant={BackgroundVariant.Dots} gap={22} size={1.4} />
        <Controls showLock={false} />
        {#if nodes.length > 0}<MiniMap pannable zoomable />{/if}
      </SvelteFlow>

      {#if nodes.length === 0}
        <div class="hint" aria-hidden="true">
          <p class="hint-title">This loop is empty</p>
          <p>Drag blocks from the left to build how your agent thinks.</p>
        </div>
      {/if}
    </div>
  </div>
</div>

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
  .name {
    width: 260px;
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
  .spacer {
    flex: 1;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .palette {
    flex: none;
    width: 232px;
    border-right: 1px solid var(--border);
    background: var(--surface);
    overflow-y: auto;
  }
  .palette-title {
    padding: var(--space-4) var(--space-4) 0;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .canvas {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  /* Map xyflow's theme onto ours. */
  .canvas :global(.svelte-flow) {
    --xy-background-color: var(--bg);
    --xy-background-pattern-color: color-mix(in srgb, var(--text-faint) 45%, transparent);
    --xy-minimap-background-color: var(--surface);
    --xy-controls-button-background-color: var(--surface);
    --xy-controls-button-background-color-hover: var(--surface-hover);
    --xy-controls-button-color: var(--text-muted);
    --xy-controls-button-border-color: var(--border);
    --xy-attribution-background-color: transparent;
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
