<script lang="ts">
  /**
   * A run's pane: every line its Pane output blocks have shown, newest
   * first, live while it runs. Shown in place of the Sessions panel's list;
   * kept on screen after the run ends until closed.
   */
  import { onMount } from "svelte";
  import { commands } from "$lib/ipc";
  import { runs } from "$lib/stores/runs.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";

  let { runId, title, chat = "", onclose }: { runId: string; title: string; chat?: string; onclose: () => void } =
    $props();

  const live = $derived(runs.runs[runId]);
  // The lines stay after the run ends (and leaves the list of runs).
  let kept = $state<string[]>([]);
  $effect(() => {
    if (live) kept = live.pane;
  });
  const working = $derived(!!live && !live.outcome);

  onMount(() => {
    commands
      .runPane(runId)
      .then((lines) => {
        runs.seedPane(runId, lines);
        if (!runs.runs[runId] && lines.length > kept.length) kept = lines;
      })
      .catch(() => {});
  });
</script>

<div class="pane" aria-label="Run pane">
  <div class="head">
    <div class="titles">
      <div class="title">{title}</div>
      <div class="sub">
        {#if working}<span class="pulse"></span>Live{:else}Finished{/if}{chat ? ` · ${chat}` : ""}
      </div>
    </div>
    <button class="close" type="button" aria-label="Close the pane" title="Close" onclick={onclose}>
      <Icon name="close" size={16} />
    </button>
  </div>

  {#if kept.length}
    <ol class="lines">
      {#each kept as line, i (kept.length - i)}
        <li class:newest={i === 0}>{line}</li>
      {/each}
    </ol>
  {:else}
    <EmptyState icon="text" title="Nothing here yet" compact>
      Lines appear here when the run's loop reaches a Pane output block, newest first.
    </EmptyState>
  {/if}
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--topbar-h);
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border);
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .sub {
    display: flex;
    align-items: center;
    gap: 5px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .pulse {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .close {
    display: grid;
    flex: none;
    place-items: center;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .close:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .lines {
    flex: 1;
    margin: 0;
    padding: var(--space-2) var(--space-3);
    overflow-y: auto;
    list-style: none;
  }
  .lines li {
    padding: 5px var(--space-2);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-size: var(--text-sm);
    line-height: 1.4;
    overflow-wrap: anywhere;
  }
  .lines li + li {
    border-top: 1px solid var(--border);
    border-radius: 0;
  }
  .lines li.newest {
    color: var(--text);
    background: var(--surface-2);
  }
</style>
