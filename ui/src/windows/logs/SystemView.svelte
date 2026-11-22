<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, events, type LogLevel, type SysEntry } from "$lib/ipc";
  import { clock } from "$lib/logs";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let days = $state<string[]>([]);
  let day = $state("");
  let entries = $state<SysEntry[]>([]);
  let error = $state("");
  let query = $state("");
  let level = $state<LogLevel>("info");
  let sources = $state<string[]>([]);
  let newest = $state(true);
  let open = $state<Record<number, boolean>>({});
  let limit = $state(500);

  const rank: Record<LogLevel, number> = { debug: 0, info: 1, warn: 2, error: 3 };
  const today = () => {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  };

  const allSources = $derived([...new Set(entries.map((e) => e.source))].sort());
  const counts = $derived.by(() => {
    const c: Record<string, number> = {};
    for (const e of entries) c[e.level] = (c[e.level] ?? 0) + 1;
    return c;
  });
  const filtered = $derived.by(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    const list = entries
      .map((e, i) => ({ e, i }))
      .filter(
        ({ e }) =>
          rank[e.level] >= rank[level] &&
          (!sources.length || sources.includes(e.source)) &&
          words.every((w) =>
            [e.message, e.source, ...Object.entries(e.fields ?? {}).flat()].join(" ").toLowerCase().includes(w),
          ),
      );
    return newest ? list.reverse() : list;
  });

  async function load(d: string) {
    error = "";
    open = {};
    limit = 500;
    try {
      entries = d ? await commands.readSystemLog(d) : [];
    } catch (e) {
      error = errorText(e);
    }
  }
  $effect(() => {
    void day;
    load(day);
  });

  onMount(() => {
    commands.systemLogDays().then((d) => {
      days = d.includes(today()) ? d : [today(), ...d];
      day = days[0] ?? "";
    });
    // New entries for today arrive live.
    const off = events.onSystemLog((e) => {
      if (day === today()) entries.push(e);
    });
    return () => off.then((f) => f());
  });

  function toggleSource(s: string) {
    sources = sources.includes(s) ? sources.filter((x) => x !== s) : [...sources, s];
  }

  const levelIcon: Record<LogLevel, "info" | "alert" | "x-circle"> = {
    debug: "info",
    info: "info",
    warn: "alert",
    error: "x-circle",
  };
</script>

<div class="sys">
  <div class="bar">
    <select bind:value={day} aria-label="Day">
      {#each days as d (d)}<option value={d}>{d === today() ? `Today (${d})` : d}</option>{/each}
    </select>
    <div class="search">
      <Icon name="search" size={14} />
      <input bind:value={query} placeholder="Search the system log…" aria-label="Search the system log" spellcheck="false" />
    </div>
    <select bind:value={level} aria-label="Lowest level shown">
      <option value="debug">Everything (with debug)</option>
      <option value="info">Info and above</option>
      <option value="warn">Warnings and errors</option>
      <option value="error">Errors only</option>
    </select>
    <select bind:value={newest} aria-label="Sort">
      <option value={true}>Newest first</option>
      <option value={false}>Oldest first</option>
    </select>
  </div>
  <div class="chips" role="group" aria-label="Sources">
    {#each allSources as s (s)}
      <button type="button" class="chip" class:on={sources.includes(s)} aria-pressed={sources.includes(s)} onclick={() => toggleSource(s)}>{s}</button>
    {/each}
    {#if sources.length}<button type="button" class="link" onclick={() => (sources = [])}>All sources</button>{/if}
    <span class="spacer"></span>
    <span class="count">
      {filtered.length} of {entries.length}
      {#if counts.warn}· <span class="warn">{counts.warn} warning{counts.warn === 1 ? "" : "s"}</span>{/if}
      {#if counts.error}· <span class="error">{counts.error} error{counts.error === 1 ? "" : "s"}</span>{/if}
    </span>
  </div>

  {#if error}
    <EmptyState icon="alert" title="Can't read this day's log">{error}</EmptyState>
  {:else if !entries.length}
    <EmptyState icon="monitor" title="Nothing logged this day">
      Chummy logs what it does here: starts, settings and permission changes, runs, jobs, tools, questions and errors.
      Never your messages, files or keys.
    </EmptyState>
  {:else}
    <ol class="rows">
      {#each filtered.slice(0, limit) as { e, i } (i)}
        {@const fields = Object.entries(e.fields ?? {})}
        <li class="row {e.level}">
          <button type="button" class="row-head" aria-expanded={!!open[i]} onclick={() => (open[i] = !open[i])} disabled={!fields.length}>
            <span class="time">{clock(e.at)}</span>
            <span class="lvl {e.level}"><Icon name={levelIcon[e.level]} size={12} />{e.level}</span>
            <span class="src">{e.source}</span>
            <span class="msg">{e.message}</span>
            {#if fields.length && !open[i]}
              <span class="kv">{fields.map(([k, v]) => `${k}: ${v}`).join(" · ")}</span>
            {/if}
          </button>
          {#if open[i] && fields.length}
            <dl class="fields" data-selectable>
              {#each fields as [k, v] (k)}
                <div>
                  <dt>{k}</dt>
                  <dd>
                    {#if k === "run"}
                      <button type="button" class="link" onclick={() => commands.openWindow("logs", `run:${v}`)}>{v}</button>
                    {:else}{v}{/if}
                  </dd>
                </div>
              {/each}
            </dl>
          {/if}
        </li>
      {/each}
    </ol>
    {#if filtered.length > limit}
      <button type="button" class="more" onclick={() => (limit += 500)}>Show more</button>
    {/if}
  {/if}
</div>

<style>
  .sys {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: var(--space-4) var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .bar {
    display: flex;
    gap: var(--space-2);
  }
  select {
    height: 32px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: var(--text-sm);
  }
  .search {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 32px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--text-faint);
  }
  .search input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .chip {
    height: 24px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-full);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .chip.on {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
    background: var(--accent-soft);
    color: var(--text);
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .spacer {
    flex: 1;
  }
  .count {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .count .warn {
    color: var(--warn);
  }
  .count .error {
    color: var(--danger);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    overflow: hidden;
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .row.warn {
    background: color-mix(in srgb, var(--warn) 5%, transparent);
  }
  .row.error {
    background: color-mix(in srgb, var(--danger) 6%, transparent);
  }
  .row-head {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    width: 100%;
    padding: 6px var(--space-3);
    border: none;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .row-head:disabled {
    cursor: default;
  }
  .time {
    flex: none;
    color: var(--text-faint);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .lvl {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    width: 58px;
    color: var(--text-faint);
    font-size: var(--text-xs);
    text-transform: uppercase;
    font-weight: 600;
  }
  .lvl.warn {
    color: var(--warn);
  }
  .lvl.error {
    color: var(--danger);
  }
  .src {
    flex: none;
    width: 76px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .msg {
    flex: none;
    max-width: 55%;
  }
  .kv {
    flex: 1;
    min-width: 0;
    color: var(--text-faint);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fields {
    display: grid;
    gap: 4px;
    margin: 0;
    padding: 0 var(--space-3) var(--space-3) calc(var(--space-3) + 200px);
    font-size: var(--text-xs);
  }
  .fields div {
    display: flex;
    gap: var(--space-2);
  }
  .fields dt {
    width: 90px;
    color: var(--text-faint);
  }
  .fields dd {
    margin: 0;
    font-family: var(--font-mono);
    word-break: break-word;
  }
  .more {
    align-self: center;
    padding: var(--space-1) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
    color: var(--text-muted);
    cursor: pointer;
  }
</style>
