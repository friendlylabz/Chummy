<script lang="ts">
  import { Handle, Position, useUpdateNodeInternals, type NodeProps } from "@xyflow/svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import { categoryColor, summary, type BlockNode } from "./loopdoc";

  let { id, data, selected }: NodeProps<BlockNode> = $props();

  const info = $derived(loops.info(data.block.type));
  const text = $derived(summary(data.block));
  const updateInternals = useUpdateNodeInternals();

  // Ports come from Rust and can change with parameters (e.g. Combine's
  // number of inputs); xyflow must re-measure the handles when they do.
  $effect(() => {
    void data.inputs.map((p) => p.name).join();
    void data.outputs.map((p) => p.name).join();
    queueMicrotask(() => updateInternals(id));
  });
</script>

<div
  class="block"
  class:selected
  class:has-issue={data.issues.length > 0}
  data-state={data.state ?? "idle"}
  style:--cat={info ? categoryColor[info.category] : "var(--text-faint)"}
>
  <div class="head">
    <span class="dot"></span>
    <span class="label">{data.block.label || info?.label || data.block.type}</span>
    {#if data.issues.length}<span class="warn" title={data.issues.join("\n")}>!</span>{/if}
  </div>
  {#if data.block.label && info && data.block.label !== info.label}
    <div class="type">{info.label}</div>
  {/if}
  {#if text}<div class="summary">{text}</div>{/if}

  {#if data.inputs.length || data.outputs.length}
    <div class="ports">
      <div class="col">
        {#each data.inputs as p (p.name)}
          <div class="port in">
            <Handle type="target" position={Position.Left} id={p.name} class="h kind-{p.kind}" />
            <span>{p.label}</span>
          </div>
        {/each}
      </div>
      <div class="col right">
        {#each data.outputs as p (p.name)}
          <div class="port out">
            <span>{p.label}</span>
            <Handle type="source" position={Position.Right} id={p.name} class="h kind-{p.kind}" />
          </div>
        {/each}
      </div>
    </div>
  {/if}

  {#if data.output && data.state}
    <div class="output" title={data.output}>{data.output}</div>
  {/if}
</div>

<style>
  .block {
    width: 220px;
    border-radius: var(--radius-lg);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-sm);
    font-size: var(--text-sm);
    transition:
      border-color var(--dur-fast) var(--ease),
      box-shadow var(--dur-fast) var(--ease);
  }
  .block.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft), var(--shadow-sm);
  }
  .block.has-issue {
    border-color: color-mix(in srgb, var(--danger) 60%, var(--border-strong));
  }
  .block[data-state="running"] {
    border-color: var(--accent);
    box-shadow: 0 0 0 4px var(--accent-soft), 0 0 24px color-mix(in srgb, var(--accent) 35%, transparent);
  }
  .block[data-state="waiting"] {
    border-color: var(--warn);
    border-style: dashed;
  }
  .block[data-state="done"] {
    border-color: color-mix(in srgb, var(--ok) 70%, var(--border-strong));
  }
  .block[data-state="failed"] {
    border-color: var(--danger);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--danger) 20%, transparent);
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3) 0;
    font-weight: 600;
  }
  .dot {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--cat);
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .warn {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--danger);
    color: white;
    font-size: 11px;
    font-weight: 700;
  }
  .type {
    padding: 0 var(--space-3);
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .summary {
    margin: 4px var(--space-3) 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
  }
  .ports {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    margin-top: var(--space-2);
    padding: var(--space-1) 0 var(--space-2);
    border-top: 1px solid var(--border);
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .col.right {
    align-items: flex-end;
  }
  .port {
    position: relative;
    display: flex;
    align-items: center;
    height: 20px;
    padding: 0 var(--space-3);
    color: var(--text-muted);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .port :global(.h) {
    width: 10px;
    height: 10px;
    border: 2px solid var(--surface);
    background: var(--text-faint);
  }
  .port :global(.h.kind-text) {
    background: #6fb3f2;
  }
  .port :global(.h.kind-list) {
    background: #a78bfa;
    border-radius: 3px;
  }
  .port :global(.h.kind-any) {
    background: var(--text-muted);
  }
  .port.in :global(.h) {
    left: -5px;
  }
  .port.out :global(.h) {
    right: -5px;
  }
  .output {
    margin: 0 var(--space-2) var(--space-2);
    padding: 4px var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
