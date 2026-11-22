<script lang="ts">
  import type { BlockInfo, Category, FunctionDef } from "$lib/ipc";
  import { loops } from "$lib/stores/loops.svelte";
  import { categoryColor, categoryLabel } from "./loopdoc";
  import Icon from "$lib/components/Icon.svelte";

  let {
    onadd,
    functions = [],
    current = null,
    onaddfunction,
    onaddnote,
  }: {
    onadd: (info: BlockInfo) => void;
    /** Add a note for people to the canvas. */
    onaddnote: () => void;
    functions?: FunctionDef[];
    /** The function being edited, which can't be added inside itself. */
    current?: string | null;
    onaddfunction: (f: FunctionDef) => void;
  } = $props();

  const order: Category[] = ["start", "think", "you", "exchange", "text", "flow", "workspace", "memory", "finish"];
  const groups = $derived(
    order
      .map((c) => ({ c, items: loops.catalog.filter((b) => b.category === c && b.palette) }))
      .filter((g) => g.items.length),
  );
  const usable = $derived(functions.filter((f) => f.id !== current));

  function dragStart(e: DragEvent, info: BlockInfo) {
    e.dataTransfer?.setData("application/x-chummy-block", info.kind);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
  }
</script>

<aside class="palette" aria-label="Blocks">
  <div class="title">Blocks</div>
  <p class="tip">Drag onto the canvas, or click to add.</p>
  {#each groups as g (g.c)}
    <div class="group">
      <div class="group-title">{categoryLabel[g.c]}</div>
      {#each g.items as b (b.kind)}
        <button
          type="button"
          class="item"
          draggable="true"
          title={b.description}
          style:--cat={categoryColor[b.category]}
          ondragstart={(e) => dragStart(e, b)}
          onclick={() => onadd(b)}
        >
          <span class="dot"></span>
          <span class="name">{b.label}</span>
        </button>
      {/each}
    </div>
  {/each}
  <div class="group">
    <div class="group-title">Notes</div>
    <button type="button" class="item" title="A note for people. It doesn't change what the loop does." onclick={onaddnote}>
      <Icon name="note" size={13} />
      <span class="name">Add note</span>
    </button>
  </div>
  <div class="group">
    <div class="group-title">Functions</div>
    {#each usable as f (f.id)}
      <button type="button" class="item" title={f.description || "A block made of other blocks"} style:--cat="var(--text-muted)" onclick={() => onaddfunction(f)}>
        <span class="dot fn"></span>
        <span class="name">{f.name}</span>
      </button>
    {:else}
      <p class="tip">Select several blocks and choose Make function to group them into one.</p>
    {/each}
  </div>
</aside>

<style>
  .palette {
    flex: none;
    width: 212px;
    overflow-y: auto;
    padding: var(--space-3) var(--space-2) var(--space-5);
    border-right: 1px solid var(--border);
    background: var(--surface);
  }
  .title {
    padding: 0 var(--space-2);
    font-weight: 650;
  }
  .tip {
    padding: 0 var(--space-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .group {
    margin-top: var(--space-3);
  }
  .group-title {
    padding: 0 var(--space-2) 2px;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .item {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    height: 30px;
    padding: 0 var(--space-2);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    text-align: left;
    font-size: var(--text-sm);
    cursor: grab;
  }
  .item:hover {
    background: var(--surface-hover);
  }
  .dot.fn {
    box-shadow: 2px 2px 0 -1px var(--surface), 2px 2px 0 0 var(--text-faint);
  }
  .dot {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--cat);
  }
</style>
