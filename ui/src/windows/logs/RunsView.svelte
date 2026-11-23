<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, events, type LogEntry, type RunListItem, type RunQuery } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { dateTime, entryOf, seconds, statusLabel } from "$lib/logs";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import RunTimeline from "$lib/components/RunTimeline.svelte";

  let { selected = $bindable(null) }: { selected: string | null } = $props();

  let text = $state("");
  let fullText = $state(false);
  let source = $state("");
  let status = $state("");
  let agent = $state("");
  let fromDay = $state("");
  let toDay = $state("");
  let sort = $state<"newest" | "oldest" | "longest" | "calls" | "steps">("newest");

  let list = $state<RunListItem[]>([]);
  let loading = $state(false);
  let error = $state("");
  let entries = $state<LogEntry[]>([]);
  let readError = $state("");

  const agents = $derived(settings.current.agents);
  const filtersOn = $derived(!!(source || status || agent || fromDay || toDay));

  const duration = (r: RunListItem) => (r.end ? r.end.ended_at - r.header.started_at : Date.now() - r.header.started_at);
  const sorted = $derived.by(() => {
    const l = [...list];
    switch (sort) {
      case "oldest":
        return l.reverse();
      case "longest":
        return l.sort((a, b) => duration(b) - duration(a));
      case "calls":
        return l.sort((a, b) => (b.end?.llm_calls ?? 0) - (a.end?.llm_calls ?? 0));
      case "steps":
        return l.sort((a, b) => (b.end?.steps ?? 0) - (a.end?.steps ?? 0));
      default:
        return l;
    }
  });
  const current = $derived(list.find((r) => r.header.run_id === selected) ?? null);
  // The log's own first and last lines, for a run the list doesn't show.
  const logHeader = $derived.by(() => {
    const e = entries.find((x) => x.type === "start");
    if (!e) return undefined;
    const { type: _t, ...h } = e as Extract<LogEntry, { type: "start" }>;
    return h;
  });
  const logEnd = $derived.by(() => {
    const e = [...entries].reverse().find((x) => x.type === "end");
    if (!e) return undefined;
    const { type: _t, ...end } = e as Extract<LogEntry, { type: "end" }>;
    return end;
  });

  let timer: ReturnType<typeof setTimeout> | undefined;
  async function search() {
    loading = true;
    error = "";
    const q: RunQuery = { text, full_text: fullText, source, status, agent_id: agent, from_day: fromDay, to_day: toDay };
    try {
      list = await commands.searchRuns(q);
    } catch (e) {
      error = errorText(e);
    } finally {
      loading = false;
    }
  }
  // Search again (after a pause in typing) whenever the query changes.
  $effect(() => {
    void [text, fullText, source, status, agent, fromDay, toDay];
    clearTimeout(timer);
    timer = setTimeout(search, 250);
  });

  async function open(id: string | null) {
    readError = "";
    entries = [];
    if (!id) return;
    try {
      entries = await commands.readRunLog(id);
    } catch (e) {
      readError = errorText(e);
    }
  }
  $effect(() => {
    void selected;
    open(selected);
  });

  onMount(() => {
    // Follow the open run while it's going, and refresh the list as runs
    // start and finish.
    const offs = [
      events.onRunEvent(({ run_id, event }) => {
        if (run_id !== selected) return;
        const e = entryOf(event);
        if (e) entries.push(e);
      }),
      events.onRunStarted(() => search()),
      events.onRunFinished(({ run_id }) => {
        search();
        if (run_id === selected) setTimeout(() => open(run_id), 300);
      }),
    ];
    return () => offs.forEach((p) => p.then((f) => f()));
  });

  function clearFilters() {
    source = status = agent = fromDay = toDay = "";
  }
</script>

<aside class="list">
  <div class="filters">
    <div class="search">
      <Icon name="search" size={14} />
      <input bind:value={text} placeholder="Find runs…" aria-label="Find runs" spellcheck="false" />
    </div>
    <label class="check"><input type="checkbox" bind:checked={fullText} /> Also search inside each log (slower)</label>
    <div class="grid">
      <select bind:value={source} aria-label="Kind of run">
        <option value="">All kinds</option>
        <option value="chat">Chats</option>
        <option value="job">Scheduled jobs</option>
        <option value="test">Test runs</option>
      </select>
      <select bind:value={status} aria-label="Status">
        <option value="">Any status</option>
        {#each Object.entries(statusLabel) as [k, v] (k)}<option value={k}>{v}</option>{/each}
      </select>
      <select bind:value={agent} aria-label="Agent">
        <option value="">All agents</option>
        {#each agents as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
      </select>
      <select bind:value={sort} aria-label="Sort">
        <option value="newest">Newest first</option>
        <option value="oldest">Oldest first</option>
        <option value="longest">Longest first</option>
        <option value="calls">Most model calls</option>
        <option value="steps">Most steps</option>
      </select>
      <label class="day">From <input type="date" bind:value={fromDay} /></label>
      <label class="day">To <input type="date" bind:value={toDay} /></label>
    </div>
    <div class="list-meta">
      <span>{loading ? "Searching…" : `${list.length} run${list.length === 1 ? "" : "s"}`}{list.length >= 500 ? " (the newest 500)" : ""}</span>
      {#if filtersOn}<button type="button" class="link" onclick={clearFilters}>Clear filters</button>{/if}
    </div>
  </div>
  {#if error}<div class="err">{error}</div>{/if}
  <ol class="runs">
    {#each sorted as r (r.header.run_id)}
      <li>
        <button type="button" class="run" class:on={r.header.run_id === selected} onclick={() => (selected = r.header.run_id)}>
          <div class="r-top">
            <span class="status {r.status}" title={statusLabel[r.status] ?? r.status}></span>
            <span class="r-title">{r.header.title}</span>
            {#if r.encrypted}<span class="lock" title={r.locked ?? "Encrypted"} class:locked={!!r.locked}><Icon name="key" size={12} /></span>{/if}
            <span class="r-when">{dateTime(r.header.started_at)}</span>
          </div>
          <div class="r-msg">{r.header.message || "(no message)"}</div>
          <div class="r-meta">
            {[
              r.header.source === "job" ? "Job" : r.header.source === "test" ? "Test" : "Chat",
              r.header.loop_name,
              seconds(duration(r)),
              r.end ? `${r.end.llm_calls} model call${r.end.llm_calls === 1 ? "" : "s"}` : "",
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
        </button>
      </li>
    {:else}
      {#if !loading}<li class="none">{text || filtersOn ? "No runs match." : "No runs yet. Every chat, job and test run is logged here."}</li>{/if}
    {/each}
  </ol>
</aside>

<main class="detail">
  {#if current || entries.length}
    {@const h = current?.header ?? logHeader}
    {@const end = current?.end ?? logEnd}
    <div class="summary">
      <div class="s-head">
        <h1>{h?.title ?? selected}</h1>
        {#if current}<span class="badge {current.status}">{statusLabel[current.status] ?? current.status}</span>{/if}
        <span class="spacer"></span>
        {#if current?.status === "running"}
          <Button variant="ghost" icon="stop" onclick={() => selected && commands.stopRun(selected)}>Stop</Button>
        {/if}
        <Button variant="ghost" icon="refresh" onclick={() => open(selected)}>Reload</Button>
      </div>
      {#if h}
        <dl class="facts">
          <div><dt>Started</dt><dd>{new Date(h.started_at).toLocaleString()}</dd></div>
          <div><dt>Took</dt><dd>{current ? seconds(duration(current)) : "—"}</dd></div>
          {#if h.agent_name}<div><dt>Agent</dt><dd>{h.agent_name}</dd></div>{/if}
          <div><dt>Loop</dt><dd>{h.loop_name}</dd></div>
          {#if h.model}<div><dt>Model</dt><dd>{h.model}</dd></div>{/if}
          {#if end}
            <div><dt>Steps</dt><dd>{end.steps}</dd></div>
            <div><dt>Model calls</dt><dd>{end.llm_calls}</dd></div>
            <div><dt>Tool calls</dt><dd>{end.tool_calls}</dd></div>
            {#if end.tokens_added}<div><dt>Tokens added</dt><dd>~{end.tokens_added.toLocaleString("en-US")}</dd></div>{/if}
            {#if end.context_window}
              <div>
                <dt>Context</dt>
                <dd>~{(end.context_tokens ?? 0).toLocaleString("en-US")} of {end.context_window.toLocaleString("en-US")}</dd>
              </div>
            {/if}
            {#if end.daily_allowed}
              <div>
                <dt>Daily usage</dt>
                <dd>{Math.round(((end.daily_used ?? 0) / end.daily_allowed) * 100)}%</dd>
              </div>
            {/if}
          {/if}
        </dl>
        {#if h.message}<div class="message" data-selectable><span class="label">Message</span>{h.message}</div>{/if}
        {#if current?.locked}<div class="error" data-selectable>{current.locked}</div>{/if}
        {#if end?.error}<div class="error" data-selectable>{end.error}{end.block ? ` (block ${end.block})` : ""}</div>{/if}
      {/if}
    </div>
    {#key selected}
      <RunTimeline {entries} />
    {/key}
  {:else if readError}
    <EmptyState icon="alert" title="Can't open this log">{readError}</EmptyState>
  {:else}
    <EmptyState icon="history" title="Pick a run">
      Every block a run executes is logged with its inputs and outputs, along with each model call, tool call and
      question.
    </EmptyState>
  {/if}
</main>

<style>
  .list {
    flex: none;
    width: 360px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    background: var(--surface);
    min-height: 0;
  }
  .filters {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border-bottom: 1px solid var(--border);
  }
  .search {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--text-faint);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  select,
  .day input {
    width: 100%;
    height: 30px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: var(--text-xs);
  }
  .day {
    display: grid;
    gap: 2px;
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .list-meta {
    display: flex;
    justify-content: space-between;
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .err {
    padding: var(--space-2) var(--space-3);
    color: var(--danger);
    font-size: var(--text-sm);
  }
  .runs {
    flex: 1;
    overflow-y: auto;
    list-style: none;
    margin: 0;
    padding: var(--space-2);
  }
  .run {
    display: grid;
    gap: 2px;
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: 1px solid transparent;
    border-radius: var(--radius-md);
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .run:hover {
    background: var(--surface-2);
  }
  .run.on {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
    background: var(--accent-soft);
  }
  .r-top {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .r-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .lock {
    color: var(--text-faint);
    display: inline-flex;
  }
  .lock.locked {
    color: var(--warn);
  }
  .r-when,
  .r-meta {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .r-msg {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .status {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .status.completed {
    background: var(--ok);
  }
  .status.failed,
  .status.interrupted {
    background: var(--danger);
  }
  .status.limit,
  .status.stopped {
    background: var(--warn);
  }
  .status.running {
    background: var(--accent);
    animation: pulse 1.2s infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .none {
    padding: var(--space-4);
    color: var(--text-faint);
    font-size: var(--text-sm);
    text-align: center;
  }
  .detail {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: var(--space-4) var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .summary {
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .s-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  h1 {
    font-size: var(--text-lg);
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .badge {
    padding: 1px var(--space-2);
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .badge.completed {
    background: color-mix(in srgb, var(--ok) 15%, transparent);
    color: var(--ok);
  }
  .badge.failed,
  .badge.interrupted {
    background: color-mix(in srgb, var(--danger) 14%, transparent);
    color: var(--danger);
  }
  .badge.limit,
  .badge.stopped {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .badge.running {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-5);
    margin: 0;
  }
  .facts dt {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .facts dd {
    margin: 0;
    font-size: var(--text-sm);
  }
  .message {
    display: grid;
    gap: 2px;
    max-height: 120px;
    overflow: auto;
    white-space: pre-wrap;
    font-size: var(--text-sm);
  }
  .label {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .error {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-size: var(--text-sm);
    white-space: pre-wrap;
  }
</style>
