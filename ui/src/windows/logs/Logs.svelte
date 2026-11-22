<script lang="ts">
  import { onMount } from "svelte";
  import { commands, events } from "$lib/ipc";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import RunsView from "./RunsView.svelte";
  import SystemView from "./SystemView.svelte";

  let tab = $state<"runs" | "system">("runs");
  /** A run to open (from `#run:<id>`). */
  let openRun = $state<string | null>(null);

  function go(section: string) {
    if (section === "system") tab = "system";
    else if (section.startsWith("run:")) {
      tab = "runs";
      openRun = section.slice(4);
    } else if (section === "runs") tab = "runs";
  }

  onMount(() => {
    go(decodeURIComponent(location.hash.replace(/^#/, "")));
    events.onGoto(go);
  });
</script>

<div class="logs">
  <header class="top">
    <div class="tabs" role="tablist">
      <button type="button" role="tab" class:on={tab === "runs"} aria-selected={tab === "runs"} onclick={() => (tab = "runs")}>
        <Icon name="flow" size={15} /> Runs
      </button>
      <button type="button" role="tab" class:on={tab === "system"} aria-selected={tab === "system"} onclick={() => (tab = "system")}>
        <Icon name="monitor" size={15} /> System log
      </button>
    </div>
    <span class="hint">Kept for 30 days, on this computer only.</span>
    <span class="spacer"></span>
    <Button variant="ghost" icon="folder" onclick={() => commands.revealLogs()}>Open logs folder</Button>
  </header>
  <div class="body">
    {#if tab === "runs"}
      <RunsView bind:selected={openRun} />
    {:else}
      <SystemView />
    {/if}
  </div>
</div>

<style>
  .logs {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .top {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    height: var(--topbar-h);
    padding: 0 var(--space-3);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 3px;
    border-radius: var(--radius-md);
    background: var(--surface-2);
  }
  .tabs button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--text-muted);
    font: inherit;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .tabs button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
    font-weight: 600;
  }
  .hint {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .spacer {
    flex: 1;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
</style>
