<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, type Job, type RunRecord } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import { jobs, blankJob } from "$lib/stores/jobs.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import AskCard from "$lib/components/AskCard.svelte";
  import JobForm from "./JobForm.svelte";

  /** The selected job's id, "new" for a job being created, or null. */
  let selected = $state<string | null>(null);
  let draft = $state<Job | null>(null);
  /** The saved version of the draft, to tell whether it changed. */
  let saved = $state("");
  let error = $state("");
  let busy = $state(false);
  let confirmDelete = $state(false);
  let openRun = $state<string | null>(null);

  const list = $derived(jobs.overview.jobs);
  const view = $derived(selected && selected !== "new" ? jobs.view(selected) : undefined);
  const dirty = $derived(draft ? JSON.stringify(draft) !== saved : false);
  const liveRun = $derived(view?.run_id ? runs.runs[view.run_id] : undefined);
  const waiting = $derived(view ? asks.forJob(view.job.id) : []);
  const noAgents = $derived(settings.current.agents.length === 0);

  onMount(() => {
    jobs.init();
    loops.init();
    runs.init();
    asks.init();
  });

  function pick(id: string | null) {
    error = "";
    openRun = null;
    selected = id;
    if (id === "new") {
      draft = blankJob(settings.current.agents[0]?.id ?? "");
      saved = "";
    } else if (id) {
      const v = jobs.view(id);
      draft = v ? structuredClone($state.snapshot(v.job)) : null;
      saved = JSON.stringify(draft);
    } else {
      draft = null;
    }
    jobs.loadHistory(id && id !== "new" ? id : null);
  }

  async function save() {
    if (!draft) return;
    busy = true;
    error = "";
    try {
      const id = await commands.saveJob($state.snapshot(draft) as Job);
      // Wait for the list to include it, then show the saved version.
      await jobs.init();
      jobs.overview = await commands.listJobs();
      pick(id);
    } catch (e) {
      error = errorText(e);
    }
    busy = false;
  }

  async function act(f: () => Promise<unknown>) {
    error = "";
    try {
      await f();
    } catch (e) {
      error = errorText(e);
    }
  }

  async function remove() {
    if (!view) return;
    confirmDelete = false;
    await act(() => commands.deleteJob(view.job.id));
    pick(null);
  }

  function when(unix?: number): string {
    if (!unix) return "";
    const d = new Date(unix * 1000);
    const today = new Date();
    const sameDay = d.toDateString() === today.toDateString();
    return sameDay
      ? `today ${d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}`
      : d.toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }

  const statusLabel: Record<RunRecord["status"], string> = {
    completed: "Done",
    failed: "Failed",
    stopped: "Stopped",
    limit: "Hit a limit",
    missed: "Missed",
    skipped: "Skipped",
  };

  function cause(r: RunRecord): string {
    switch (r.cause.kind) {
      case "manual":
        return "Started by hand";
      case "catch_up":
        return "Caught up on a missed time";
      case "file":
        return `${r.cause.path} changed`;
      default:
        return "On schedule";
    }
  }

  function stateText(v: NonNullable<typeof view>): string {
    if (v.state === "running") return "Running now";
    if (v.state === "queued") return "Waiting to start (its agent is busy)";
    if (!v.job.enabled) return "Switched off · runs only by hand";
    if (v.next_run) return `Next run ${when(v.next_run)}`;
    if (v.job.triggers.some((t) => t.kind === "folder")) return "Watching for files";
    return v.job.triggers.length ? "Nothing more scheduled" : "No schedule · runs only by hand";
  }
</script>

<div class="layout">
  <aside class="side">
    <div class="side-head">
      <span class="side-title">Jobs</span>
      <div class="side-actions">
        <Button variant="ghost" icon="import" iconOnly aria-label="Import a job" title="Import a job" onclick={() => act(async () => {
          const id = await commands.importJob();
          if (id) {
            jobs.overview = await commands.listJobs();
            pick(id);
          }
        })} />
        <Button variant="primary" icon="plus" iconOnly aria-label="New job" title="New job" disabled={noAgents} onclick={() => pick("new")} />
      </div>
    </div>
    <div class="list">
      {#each list as v (v.job.id)}
        <button type="button" class="item" class:current={selected === v.job.id} onclick={() => pick(v.job.id)}>
          <span class="dot {v.state}" class:off={!v.job.enabled}></span>
          <span class="item-text">
            <span class="item-name">{v.job.name}</span>
            <span class="item-sub">{v.schedule[0] ?? "By hand"}{v.schedule.length > 1 ? ` +${v.schedule.length - 1}` : ""}</span>
          </span>
          {#if asks.forJob(v.job.id).length}<Icon name="ask" size={14} />{/if}
        </button>
      {:else}
        <p class="side-empty">No jobs yet.</p>
      {/each}
      {#each jobs.overview.bad_files as b (b.file)}
        <div class="bad" title={b.error}><Icon name="alert" size={13} /> {b.file} can't be read</div>
      {/each}
    </div>
    <div class="side-foot">
      <div>
        <div class="foot-title">Pause scheduled jobs</div>
        <div class="foot-sub">Times that pass while paused are skipped.</div>
      </div>
      <Toggle checked={jobs.overview.paused} label="Pause scheduled jobs" onchange={(v) => act(() => commands.setJobsPaused(v))} />
    </div>
  </aside>

  <main class="content">
    {#if !draft}
      <div class="empty">
        <EmptyState icon="clock" title={list.length ? "Pick a job" : "No jobs yet"}>
          A job runs one of your agents with a message: at set times, every so often, or when files land in a folder of
          the agent's. It can do exactly what the agent can, and asks you in chat when it needs you.
          {#snippet actions()}
            {#if noAgents}
              <Button icon="chat" onclick={() => commands.openWindow("settings", "agents")}>Set up an agent first</Button>
            {:else}
              <Button variant="primary" icon="plus" onclick={() => pick("new")}>New job</Button>
            {/if}
          {/snippet}
        </EmptyState>
      </div>
    {:else}
      <div class="inner">
        <header class="head">
          <div class="head-text">
            <h1>{selected === "new" ? "New job" : view?.job.name}</h1>
            {#if view}
              <p class="sub">
                {#if view.state === "running"}<span class="pulse"></span>{/if}
                {stateText(view)}{liveRun?.currentLabel ? ` · ${liveRun.currentLabel}` : ""}
              </p>
              {#if view.watch_error}<p class="warn"><Icon name="alert" size={13} /> Can't watch {view.watch_error}</p>{/if}
            {/if}
          </div>
          {#if view}
            <div class="head-actions">
              <Toggle checked={view.job.enabled} label="Switched on" onchange={(v) => act(async () => {
                await commands.setJobEnabled(view.job.id, v);
                if (draft) draft.enabled = v;
                saved = JSON.stringify({ ...JSON.parse(saved), enabled: v });
              })} />
              {#if view.state === "idle"}
                <Button icon="play" disabled={dirty} title={dirty ? "Save your changes first" : ""} onclick={() => act(() => commands.runJobNow(view.job.id))}>Run now</Button>
              {:else}
                <Button icon="stop" onclick={() => act(() => commands.stopJob(view.job.id))}>Stop</Button>
              {/if}
              <Button variant="ghost" icon="export" iconOnly aria-label="Export" title="Export" onclick={() => act(() => commands.exportJob(view.job.id))} />
              <Button variant="ghost" icon="trash" iconOnly aria-label="Delete" title="Delete" onclick={() => (confirmDelete = true)} />
            </div>
          {/if}
        </header>

        {#each waiting as p (p.id)}
          <div class="ask"><AskCard pending={p} showOrigin={false} /></div>
        {/each}

        {#if error}<div class="error" role="alert">{error}</div>{/if}

        <JobForm bind:draft />

        <div class="save-bar">
          {#if selected === "new"}
            <Button onclick={() => pick(null)}>Cancel</Button>
          {:else if dirty}
            <Button onclick={() => pick(selected)}>Undo changes</Button>
          {/if}
          <Button variant="primary" icon="check" disabled={busy || (!dirty && selected !== "new")} onclick={save}>
            {selected === "new" ? "Create job" : "Save"}
          </Button>
        </div>

        {#if view}
          <h2>Runs</h2>
          <div class="runs">
            {#each jobs.history as r (r.run_id)}
              <div class="run {r.status}">
                <button type="button" class="run-head" onclick={() => (openRun = openRun === r.run_id ? null : r.run_id)} aria-expanded={openRun === r.run_id}>
                  <span class="badge">{statusLabel[r.status]}</span>
                  <span class="run-when">{when(r.started_at)}</span>
                  <span class="run-cause">{cause(r)}</span>
                  {#if r.llm_calls}<span class="run-meta">{r.llm_calls} model call{r.llm_calls === 1 ? "" : "s"}</span>{/if}
                  <Icon name="chevron-down" size={14} />
                </button>
                {#if openRun === r.run_id}
                  <div class="run-body" data-selectable>
                    {#if r.note}<p class="note">{r.note}</p>{/if}
                    {#if r.reply}<div class="reply">{r.reply}</div>{/if}
                    {#if !r.note && !r.reply}<p class="note">Nothing to show.</p>{/if}
                    {#if r.status !== "missed" && r.status !== "skipped"}
                      <button type="button" class="log-link" onclick={() => commands.openWindow("logs", `run:${r.run_id}`)}>
                        <Icon name="history" size={13} /> See every step in Logs
                      </button>
                    {/if}
                  </div>
                {/if}
              </div>
            {:else}
              <p class="none">No runs yet.</p>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </main>
</div>

{#if confirmDelete && view}
  <Modal title="Delete “{view.job.name}”?" onclose={() => (confirmDelete = false)}>
    <p>Its schedule and run history go too. The agent and its folder aren't touched.</p>
    {#snippet actions()}
      <Button onclick={() => (confirmDelete = false)}>Cancel</Button>
      <Button variant="primary" icon="trash" onclick={remove}>Delete</Button>
    {/snippet}
  </Modal>
{/if}

<style>
  .log-link {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-top: var(--space-2);
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .layout {
    display: flex;
    height: 100%;
  }
  .side {
    flex: none;
    width: 260px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }
  .side-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-4) var(--space-3) var(--space-2) var(--space-5);
  }
  .side-title {
    font-weight: 650;
  }
  .side-actions {
    display: flex;
    gap: var(--space-1);
  }
  .list {
    flex: 1;
    overflow-y: auto;
    padding: 0 var(--space-2);
  }
  .item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    text-align: left;
    cursor: pointer;
  }
  .item:hover {
    background: var(--surface-hover);
  }
  .item.current {
    background: var(--accent-soft);
  }
  .item :global(svg) {
    color: var(--accent);
  }
  .item-text {
    flex: 1;
    min-width: 0;
    display: grid;
  }
  .item-name {
    font-weight: 550;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-sub {
    color: var(--text-muted);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok);
  }
  .dot.off {
    background: var(--border-strong);
  }
  .dot.running,
  .dot.queued {
    background: var(--accent);
    animation: pulse 1.2s var(--ease) infinite;
  }
  .side-empty {
    padding: var(--space-3);
    color: var(--text-faint);
    font-size: var(--text-sm);
  }
  .bad {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    padding: var(--space-2) var(--space-3);
    color: var(--warn);
    font-size: var(--text-xs);
  }
  .side-foot {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--border);
  }
  .foot-title {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .foot-sub {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
  }
  .empty {
    display: flex;
    height: 100%;
    padding: var(--space-6);
  }
  .inner {
    max-width: 780px;
    padding: var(--space-6);
    display: grid;
    gap: var(--space-4);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
  }
  .head-text {
    flex: 1;
    min-width: 0;
  }
  .head-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
    overflow-wrap: anywhere;
  }
  h2 {
    margin-top: var(--space-4);
    font-size: var(--text-md);
    font-weight: 650;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-1);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .warn {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin-top: var(--space-1);
    color: var(--warn);
    font-size: var(--text-sm);
  }
  .pulse {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .error {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    color: var(--danger);
    font-size: var(--text-sm);
  }
  .save-bar {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
  .runs {
    display: grid;
    gap: var(--space-2);
  }
  .run {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
  }
  .run-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: none;
    background: transparent;
    text-align: left;
    cursor: pointer;
    font-size: var(--text-sm);
  }
  .badge {
    flex: none;
    padding: 1px 8px;
    border-radius: var(--radius-full);
    background: color-mix(in srgb, var(--ok) 18%, transparent);
    color: var(--ok);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .run.failed .badge,
  .run.limit .badge {
    background: color-mix(in srgb, var(--danger) 15%, transparent);
    color: var(--danger);
  }
  .run.stopped .badge,
  .run.missed .badge,
  .run.skipped .badge {
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .run-when {
    flex: none;
  }
  .run-cause {
    flex: 1;
    min-width: 0;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .run-meta {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .run-body {
    padding: 0 var(--space-3) var(--space-3);
    display: grid;
    gap: var(--space-2);
  }
  .note {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .reply {
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.55;
    padding: var(--space-2) var(--space-3);
    border-left: 2px solid var(--border-strong);
  }
  .none {
    color: var(--text-faint);
    font-size: var(--text-sm);
  }
</style>
