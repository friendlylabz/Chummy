<script lang="ts">
  import type { LoopReview } from "$lib/ipc";
  import Icon from "$lib/components/Icon.svelte";
  import FlowTree from "./FlowTree.svelte";
  import { blockName, originLabel, originTone } from "./names";

  let {
    review,
    step,
    title = "",
    port = "",
    path = [],
    depth = 0,
  }: {
    review: LoopReview;
    step: number;
    /** For the first step of a flow. */
    title?: string;
    /** The input of the step above that this one feeds. */
    port?: string;
    /** Steps above this one, to stop when a cycle comes round. */
    path?: number[];
    depth?: number;
  } = $props();

  const s = $derived(review.steps[step]);
  const again = $derived(path.includes(step));
  const feeds = $derived(s.inputs.reduce((n, i) => n + i.from.length, 0));
  // The first two steps start open; deeper ones on request.
  let open = $state(false);
  $effect.pre(() => {
    open = depth < 2;
  });
  const here = $derived([...path, step]);
  const what = $derived.by(() => {
    if (!s.port || s.port === "out") return "";
    if (s.port.startsWith("input “") || s.port.startsWith("memory “")) return s.port;
    return `its “${s.port}” output`;
  });
</script>

<div class="step" class:root={depth === 0}>
  <div class="line">
    {#if feeds && !again}
      <button class="toggle" type="button" aria-expanded={open} aria-label={open ? "Close" : "Open"} onclick={() => (open = !open)}>
        <span class="chev" class:open><Icon name="chevron-down" size={14} /></span>
      </button>
    {:else}
      <span class="dot"></span>
    {/if}
    <div class="what">
      {#if title}<span class="title">{title}:</span>{/if}
      <span class="name">{blockName(review.blocks[s.block])}</span>
      {#if port}<span class="via">into “{port}”</span>{/if}
      {#if what}<span class="via">({what})</span>{/if}
      {#if again}<span class="via">↻ comes round again (a cycle)</span>{/if}
    </div>
    <div class="origins">
      {#each s.origins as o (o)}<span class="origin {originTone(o)}">{originLabel(o)}</span>{/each}
    </div>
  </div>
  {#if open && !again}
    <div class="children">
      {#each s.inputs as i (i.port)}
        {#each i.from as f (f)}
          <FlowTree {review} step={f} port={i.label} path={here} depth={depth + 1} />
        {/each}
      {/each}
    </div>
  {/if}
</div>

<style>
  .step.root {
    margin-bottom: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .line {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    padding: 3px 0;
    font-size: var(--text-sm);
  }
  .toggle {
    flex: none;
    width: 18px;
    height: 18px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    align-self: center;
  }
  .chev {
    display: inline-grid;
    transform: rotate(-90deg);
    transition: transform var(--dur-fast) var(--ease);
  }
  .chev.open {
    transform: none;
  }
  .dot {
    flex: none;
    width: 18px;
    text-align: center;
    color: var(--text-faint);
    align-self: center;
  }
  .dot::before {
    content: "·";
  }
  .what {
    min-width: 0;
    flex: 1;
  }
  .title {
    font-weight: 650;
    margin-right: var(--space-1);
  }
  .name {
    font-weight: 550;
  }
  .via {
    margin-left: var(--space-1);
    color: var(--text-muted);
  }
  .origins {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 3px;
    max-width: 45%;
  }
  .origin {
    padding: 0 6px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .origin.watch {
    background: color-mix(in srgb, var(--warn) 14%, transparent);
    color: var(--warn);
  }
  .children {
    margin-left: 8px;
    padding-left: var(--space-3);
    border-left: 1px solid var(--border);
  }
</style>
