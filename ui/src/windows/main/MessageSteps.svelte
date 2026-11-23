<script lang="ts">
  /**
   * What the agent did for one message: its model calls (with reasoning),
   * tool calls, questions and, on request, every block step. Collapsed until
   * opened; the log is read when first opened.
   */
  import { commands, errorText, type LogEntry } from "$lib/ipc";
  import type { Group } from "$lib/logs";
  import Icon from "$lib/components/Icon.svelte";
  import RunTimeline from "$lib/components/RunTimeline.svelte";

  let {
    runId,
    live = null,
    summary = "",
  }: {
    runId: string;
    /** Entries of a run still going (from the runs store). */
    live?: LogEntry[] | null;
    /** e.g. "3 model calls · 40 steps". */
    summary?: string;
  } = $props();

  let open = $state(false);
  let loaded = $state<LogEntry[] | null>(null);
  let error = $state("");
  const entries = $derived(live ?? loaded ?? []);
  const groups: Group[] = ["model", "tools", "you", "notices"];

  async function toggle() {
    open = !open;
    if (open && !live && !loaded) {
      try {
        loaded = await commands.readRunLog(runId);
      } catch (e) {
        error = errorText(e);
      }
    }
  }
</script>

<div class="steps">
  <div class="head">
    <button type="button" class="toggle" aria-expanded={open} onclick={toggle}>
      <Icon name={open ? "chevron-down" : "flow"} size={13} />
      {open ? "Hide what happened" : "What happened"}
    </button>
    {#if summary}<span class="sum">{summary}</span>{/if}
    {#if open}
      <button type="button" class="logs" onclick={() => commands.openWindow("logs", `run:${runId}`)}>
        Open in Logs <Icon name="external" size={12} />
      </button>
    {/if}
  </div>
  {#if open}
    <div class="panel">
      {#if error}
        <p class="err">{error}</p>
      {:else if !live && !loaded}
        <p class="wait">Reading the log…</p>
      {:else}
        <RunTimeline {entries} compact {groups} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .steps {
    margin-top: var(--space-1);
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px var(--space-3);
  }
  .toggle,
  .logs {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-faint);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .toggle:hover,
  .logs:hover {
    color: var(--accent);
  }
  .logs {
    margin-left: auto;
    color: var(--accent);
  }
  .sum {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .panel {
    min-width: 0;
    overflow: hidden;
    margin-top: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .wait,
  .err {
    color: var(--text-faint);
    font-size: var(--text-sm);
  }
  .err {
    color: var(--danger);
  }
</style>
