<script lang="ts">
  import type { BlockInfo, Category } from "$lib/ipc";
  import { loops } from "$lib/stores/loops.svelte";
  import { categoryColor, categoryLabel } from "./loopdoc";

  let { onadd }: { onadd: (info: BlockInfo) => void } = $props();

  const order: Category[] = ["start", "think", "text", "flow", "workspace", "finish"];
  const groups = $derived(
    order.map((c) => ({ c, items: loops.catalog.filter((b) => b.category === c) })).filter((g) => g.items.length),
  );

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
  .dot {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--cat);
  }
</style>
