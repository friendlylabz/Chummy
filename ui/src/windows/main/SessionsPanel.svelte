<script lang="ts">
  import { onMount } from "svelte";
  import { commands } from "$lib/ipc";
  import { runs } from "$lib/stores/runs.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import RunPane from "./RunPane.svelte";

  let { open = $bindable(true) }: { open: boolean } = $props();

  const agents = $derived(settings.current.agents);

  // Runs change many times a second (each block that starts or ends). The
  // panel never follows them directly: every half second it reads the queued
  // status updates (runs.takeStatus) and redraws only what changed, so it
  // doesn't flicker as blocks race by or as a run finishes.
  interface Row {
    id: string;
    title: string;
    /** The chat, for chat runs. */
    chat: string;
    agent: string | null;
    chatId: string | null;
    step: string;
  }
  let active = $state<Row[]>([]);
  /** The run whose pane is shown instead of the lists, if any. */
  let paneFor = $state<Row | null>(null);
  let busyAgents = $state<string[]>([]);

  const stepText = (s: string) => (s === "Waiting for you" ? s : `${s}…`);

  function snapshot() {
    const status = runs.takeStatus(500);
    const next: Row[] = runs.active.map((r) => ({
      id: r.info.run_id,
      title: `${r.info.source === "job" ? "Job: " : ""}${r.info.title}`,
      chat: r.info.chat_title ?? "",
      agent: r.info.agent_id,
      chatId: r.info.chat_id ?? null,
      step: stepText(status.get(r.info.run_id) ?? "Working"),
    }));
    if (JSON.stringify(next) !== JSON.stringify(active)) active = next;
    const busy = agents.filter((a) => runs.forAgent(a.id)).map((a) => a.id);
    if (busy.join() !== busyAgents.join()) busyAgents = busy;
  }

  /** Opens a chat (and its agent). */
  async function openChat(agentId: string | null | undefined, chatId: string | null | undefined) {
    if (!agentId) return;
    if (chatId) await commands.setActiveChat(agentId, chatId).catch(() => {});
    if (agentId !== settings.current.active_agent) await commands.setActiveAgent(agentId);
  }

  onMount(() => {
    snapshot();
    const t = setInterval(snapshot, 500);
    return () => clearInterval(t);
  });
</script>

<aside class="panel" class:open aria-label="Sessions" aria-hidden={!open} inert={!open}>
  {#if paneFor}
    <div class="inner">
      <RunPane runId={paneFor.id} title={paneFor.title} chat={paneFor.chat} onclose={() => (paneFor = null)} />
    </div>
  {:else}
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
                : openChat(p.origin.agent_id, p.origin.chat_id)}
          >
            <Icon name={p.ask.kind === "tool_approval" ? "shield" : p.ask.kind === "budget" ? "gauge" : "ask"} size={15} />
            <span class="w-text">
              <span class="w-title">{p.origin.title}</span>
              <span class="w-sub">{p.ask.kind === "tool_approval"
                  ? `Allow ${p.ask.tool}?`
                  : p.ask.kind === "budget"
                    ? `${p.ask.model}: daily tokens used`
                    : p.ask.text}</span>
            </span>
          </button>
        {/each}
      </div>
    {/if}

    <div class="group">
      <div class="group-title">Running</div>
      {#each active as r (r.id)}
        <div class="run">
          <span class="pulse"></span>
          <button class="run-text" type="button" disabled={!r.chatId} title={r.chatId ? "Open this chat" : undefined} onclick={() => openChat(r.agent, r.chatId)}>
            <div class="run-title">{r.title}</div>
            {#if r.chat}<div class="run-chat"><Icon name="chat" size={12} /><span>{r.chat}</span></div>{/if}
            <div class="run-step">{r.step}</div>
          </button>
          <button class="icon-btn" type="button" aria-label="Show the run's pane" title="Show what it's doing (its pane)" onclick={() => (paneFor = r)}>
            <Icon name="text" size={14} />
          </button>
          <button class="icon-btn" type="button" aria-label="Stop" title="Stop" onclick={() => commands.stopRun(r.id)}>
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
          {#if busyAgents.includes(a.id)}<span class="dot"></span>{/if}
        </button>
      {:else}
        <EmptyState icon="bot" title="No agents yet" compact />
      {/each}
    </div>
  </div>
  {/if}
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
    padding: 0;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
  }
  .run-text:not(:disabled) {
    cursor: pointer;
  }
  .run-text:not(:disabled):hover .run-title {
    color: var(--accent);
  }
  .run-chat {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-muted);
    font-size: var(--text-xs);
    min-width: 0;
  }
  .run-chat span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    height: auto;
    padding-top: 5px;
    padding-bottom: 5px;
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
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
