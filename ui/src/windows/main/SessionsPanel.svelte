<script lang="ts">
  import { commands } from "$lib/ipc";
  import { runs } from "$lib/stores/runs.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let { open = $bindable(true) }: { open: boolean } = $props();

  const active = $derived(runs.active);
  const agents = $derived(settings.current.agents);
</script>

<aside class="panel" class:open aria-label="Sessions" aria-hidden={!open} inert={!open}>
  <div class="inner">
    <div class="head">
      <span class="title">Sessions</span>
      <button class="collapse" type="button" aria-label="Hide sessions" onclick={() => (open = false)}>
        <Icon name="panel-right" size={16} />
      </button>
    </div>

    {#if asks.pending.length}
      <div class="group">
        <div class="group-title">Waiting for you</div>
        {#each asks.pending as p (p.id)}
          <button
            class="agent waiting"
            type="button"
            title={p.ask.text}
            onclick={() =>
              p.origin.kind === "test"
                ? commands.openWindow("editor")
                : p.origin.agent_id && commands.setActiveAgent(p.origin.agent_id)}
          >
            <Icon name={p.ask.kind === "tool_approval" ? "shield" : "ask"} size={15} />
            <span class="w-text">
              <span class="w-title">{p.origin.title}</span>
              <span class="w-sub">{p.ask.kind === "tool_approval" ? `Allow ${p.ask.tool}?` : p.ask.text}</span>
            </span>
          </button>
        {/each}
      </div>
    {/if}

    <div class="group">
      <div class="group-title">Running</div>
      {#each active as r (r.info.run_id)}
        <div class="run">
          <span class="pulse"></span>
          <div class="run-text">
            <div class="run-title">{r.info.source === "job" ? "Job: " : ""}{r.info.title}</div>
            <div class="run-step">{r.waitingForYou ? "Waiting for you" : r.currentLabel || "Starting…"}</div>
          </div>
          <button class="icon-btn" type="button" aria-label="Stop" title="Stop" onclick={() => commands.stopRun(r.info.run_id)}>
            <Icon name="stop" size={14} />
          </button>
        </div>
      {:else}
        <EmptyState icon="play" title="No agents running" compact>
          Chats, test runs and scheduled jobs show up here while they work.
        </EmptyState>
      {/each}
    </div>

    <div class="group">
      <div class="group-title">Agents</div>
      {#each agents as a (a.id)}
        <button
          class="agent"
          class:current={a.id === settings.current.active_agent}
          type="button"
          onclick={() => commands.setActiveAgent(a.id)}
        >
          <Icon name="bot" size={15} />
          <span>{a.name}</span>
          {#if runs.forAgent(a.id)}<span class="dot"></span>{/if}
        </button>
      {:else}
        <EmptyState icon="bot" title="No agents yet" compact />
      {/each}
    </div>
  </div>
</aside>

<style>
  .panel {
    flex: none;
    width: 0;
    overflow: hidden;
    border-left: 1px solid transparent;
    background: var(--surface);
    transition:
      width var(--dur-med) var(--ease),
      border-color var(--dur-med) var(--ease);
  }
  .panel.open {
    width: var(--sessions-w);
    border-left-color: var(--border);
  }
  .inner {
    width: var(--sessions-w);
    height: 100%;
    overflow-y: auto;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: var(--topbar-h);
    padding: 0 var(--space-2) 0 var(--space-4);
  }
  .title {
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .collapse {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .collapse:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .group {
    padding: var(--space-2) var(--space-2) 0;
  }
  .run {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: var(--space-2) 0 0;
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--surface-2);
  }
  .run-text {
    flex: 1;
    min-width: 0;
  }
  .run-title {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .run-step {
    color: var(--text-muted);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pulse,
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .pulse {
    animation: pulse 1.2s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .icon-btn {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .agent {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    height: 32px;
    margin-top: 2px;
    padding: 0 var(--space-2);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .agent span:not(.dot) {
    flex: 1;
  }
  .agent:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .agent.current {
    color: var(--text);
    font-weight: 550;
  }
  .group-title {
    padding: var(--space-2) var(--space-2) 0;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .waiting {
    align-items: flex-start;
  }
  .waiting :global(svg) {
    margin-top: 2px;
    color: var(--accent);
  }
  .w-text {
    display: grid;
    min-width: 0;
    text-align: left;
  }
  .w-title {
    font-weight: 550;
  }
  .w-sub {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
</style>
