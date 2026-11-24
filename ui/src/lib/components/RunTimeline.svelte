<script lang="ts">
  /**
   * A run's log as a timeline: every block step with its inputs and outputs,
   * every model call, tool call, question, notice and reply. Rows open to
   * show the full text. Search, filter by kind, and sort.
   */
  import { onMount } from "svelte";
  import type { BlockInfo, Category, LogEntry, LoggedValue } from "$lib/ipc";
  import { categoryColor, categoryLabel } from "$lib/blocks";
  import { blockCatalog, groupLabel, offset, seconds, timeline, type Group, type Item } from "$lib/logs";
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    entries,
    compact = false,
    groups: initialGroups,
    sort: initialSort = "order",
  }: {
    entries: LogEntry[];
    /** Smaller, for the chat. */
    compact?: boolean;
    /** Kinds shown at first. Default: everything. */
    groups?: Group[];
    /** How rows are sorted at first. Default: in order (oldest first). */
    sort?: "order" | "newest" | "slowest";
  } = $props();

  let catalog = $state<BlockInfo[]>([]);
  onMount(async () => {
    catalog = await blockCatalog();
  });

  const allGroups: Group[] = ["model", "tools", "you", "replies", "notices", "blocks"];
  // svelte-ignore state_referenced_locally
  let shown = $state<Group[]>(initialGroups ?? allGroups);
  let query = $state("");
  let category = $state<Category | "">("");
  let failuresOnly = $state(false);
  // svelte-ignore state_referenced_locally
  let sort = $state<"order" | "newest" | "slowest">(initialSort);
  let open = $state<Record<number, boolean>>({});
  let limit = $state(300);

  const t = $derived(timeline(entries, catalog));
  const start = $derived(t.header?.started_at ?? t.items[0]?.at ?? 0);
  const counts = $derived.by(() => {
    const c: Record<string, number> = {};
    for (const i of t.items) c[i.group] = (c[i.group] ?? 0) + 1;
    return c;
  });
  const categories = $derived([...new Set(t.items.map((i) => i.category).filter((c): c is Category => !!c))]);

  const filtered = $derived.by(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    let list = t.items.filter(
      (i) =>
        shown.includes(i.group) &&
        (!category || i.group !== "blocks" || i.category === category) &&
        (!failuresOnly || i.failed) &&
        words.every((w) => i.haystack.includes(w)),
    );
    if (sort === "newest") list = [...list].reverse();
    if (sort === "slowest") list = [...list].sort((a, b) => (b.ms ?? -1) - (a.ms ?? -1));
    return list;
  });
  const visible = $derived(filtered.slice(0, limit));
  /** Kinds that have entries but are switched off, e.g. "block steps (140)". */
  const hidden = $derived(
    allGroups.filter((g) => counts[g] && !shown.includes(g)).map((g) => `${groupLabel[g].toLowerCase()} (${counts[g]})`),
  );
  const countLine = $derived(
    [
      filtered.length === t.items.length ? `${t.items.length} entries` : `${filtered.length} of ${t.items.length} entries`,
      sort === "newest" ? "newest first" : "",
      hidden.length ? `hidden: ${hidden.join(", ")} (click a chip above to show them)` : "",
    ]
      .filter(Boolean)
      .join(" · "),
  );

  function toggle(g: Group) {
    shown = shown.includes(g) ? shown.filter((x) => x !== g) : [...shown, g];
  }
  function only(g: Group) {
    shown = [g];
  }
  function expandAll(on: boolean) {
    open = on ? Object.fromEntries(visible.map((i) => [i.i, true])) : {};
  }

  const icon: Record<Group, IconName> = {
    model: "bot",
    tools: "wrench",
    you: "ask",
    replies: "chat",
    notices: "info",
    blocks: "flow",
  };

  function meta(v: LoggedValue): string {
    const parts: string[] = [v.kind];
    if (v.items != null) parts.push(`${v.items} ${v.kind === "exchange" ? "messages" : "items"}`);
    parts.push(`${v.chars.toLocaleString()} characters`);
    if (v.cut) parts.push(`${v.cut.toLocaleString()} left out of this log`);
    return parts.join(" · ");
  }

  function dotColor(i: Item): string {
    if (i.group === "blocks") return i.category ? categoryColor[i.category] : "var(--text-faint)";
    return {
      model: "var(--accent)",
      tools: "#7fd1a4",
      you: "#5ec8c8",
      replies: "#6fb3f2",
      notices: "var(--warn)",
      blocks: "var(--text-faint)",
    }[i.group];
  }
</script>

{#snippet value(label: string, v: LoggedValue)}
  <div class="val">
    <div class="val-head"><span class="port">{label}</span><span class="val-meta">{meta(v)}</span></div>
    {#if v.text.trim()}<pre data-selectable>{v.text}</pre>{:else}<div class="empty">(empty)</div>{/if}
  </div>
{/snippet}

<div class="timeline" class:compact>
  <div class="bar">
    <div class="search">
      <Icon name="search" size={14} />
      <input bind:value={query} placeholder="Search this run…" aria-label="Search this run" spellcheck="false" />
      {#if query}<button type="button" class="clear" aria-label="Clear search" onclick={() => (query = "")}><Icon name="close" size={12} /></button>{/if}
    </div>
    <select bind:value={sort} aria-label="Sort">
      <option value="order">In order</option>
      <option value="newest">Newest first</option>
      <option value="slowest">Slowest first</option>
    </select>
  </div>
  <div class="chips" role="group" aria-label="Show">
    {#each allGroups as g (g)}
      {#if counts[g]}
        <button
          type="button"
          class="chip"
          class:on={shown.includes(g)}
          aria-pressed={shown.includes(g)}
          title="Click to show or hide; double-click to show only these"
          onclick={() => toggle(g)}
          ondblclick={() => only(g)}
        >
          <Icon name={icon[g]} size={12} />{groupLabel[g]} <span class="n">{counts[g]}</span>
        </button>
      {/if}
    {/each}
    {#if shown.includes("blocks") && categories.length > 1}
      <select class="cat" bind:value={category} aria-label="Block category">
        <option value="">All block types</option>
        {#each categories as c (c)}<option value={c}>{categoryLabel[c]}</option>{/each}
      </select>
    {/if}
    <label class="fail"><input type="checkbox" bind:checked={failuresOnly} /> Failures only</label>
    <span class="spacer"></span>
    <span class="links">
      <button type="button" class="link" onclick={() => expandAll(true)}>Open all</button>
      <button type="button" class="link" onclick={() => expandAll(false)}>Close all</button>
    </span>
  </div>

  {#if t.unreadable}
    <div class="locked" role="alert">
      <Icon name="key" size={14} />
      <span>{t.unreadable.lines} line{t.unreadable.lines === 1 ? "" : "s"} can't be read. {t.unreadable.reason}</span>
    </div>
  {/if}
  <div class="count">{countLine}</div>

  <ol class="rows">
    {#each visible as i (i.i)}
      {@const e = i.entry}
      <li class="row" class:failed={i.failed} class:open={open[i.i]} style:--depth={sort === "slowest" ? 0 : i.depth}>
        <button type="button" class="row-head" aria-expanded={!!open[i.i]} onclick={() => (open[i.i] = !open[i.i])}>
          <span class="time">{offset(i.at, start)}</span>
          <span class="dot" style:background={dotColor(i)}></span>
          <span class="title">{i.title}</span>
          {#if e.type === "step"}
            <span class="kind">{e.function ? `Function: ${e.function}` : (catalog.find((b) => b.kind === e.kind)?.label ?? e.kind)}</span>
          {/if}
          {#if i.block && e.type !== "step"}<span class="path">{i.block}</span>{/if}
          <span class="preview">{i.preview}</span>
          {#if i.ms != null}<span class="ms">{seconds(i.ms)}</span>{/if}
          {#if e.type === "step" && e.status !== "done"}<span class="badge {e.status}">{e.status}</span>{/if}
        </button>
        {#if open[i.i]}
          <div class="detail">
            {#if e.type === "step"}
              <div class="facts">
                <span>Step {e.n}</span><span class="mono">{e.block}</span>
                {#if catalog.find((b) => b.kind === e.kind)?.category}
                  {@const c = catalog.find((b) => b.kind === e.kind)!.category}
                  <span>{categoryLabel[c]}</span>
                {/if}
                <span>{seconds(e.ms)}</span>
              </div>
              {#if e.error}<div class="error" data-selectable>{e.error}</div>{/if}
              {#if e.inputs.length}
                <div class="sect">Inputs</div>
                {#each e.inputs as p (p.port)}{@render value(p.port, p.value)}{/each}
              {/if}
              {#if e.outputs.length}
                <div class="sect">Outputs</div>
                {#each e.outputs as p, k (k)}{@render value(p.port, p.value)}{/each}
              {:else if !e.error}
                <div class="empty">No outputs.</div>
              {/if}
            {:else if e.type === "model"}
              <div class="facts">
                <span class="mono">{e.block}</span>
                <span>{e.model ?? "The agent's model"}</span>
                <span>Tools: {e.tool_mode}{e.tools?.length ? ` (${e.tools.join(", ")})` : ""}</span>
                {#if e.attempt > 1}<span>Attempt {e.attempt}</span>{/if}
                <span>{seconds(e.ms)}</span>
              </div>
              {#if e.reasoning}
                <div class="sect">Reasoning</div>
                <pre class="reasoning" data-selectable>{e.reasoning}</pre>
              {/if}
              {#if e.calls?.length}
                <div class="sect">Tool calls</div>
                <pre data-selectable>{e.calls.join("\n")}</pre>
              {/if}
              <div class="sect">Answer</div>
              {@render value("answer", e.answer)}
              <details class="sent">
                <summary>What was sent to the model</summary>
                {@render value("conversation", e.sent)}
              </details>
            {:else if e.type === "tool_call"}
              <div class="sect">Arguments</div>
              <pre data-selectable>{e.arguments}</pre>
            {:else if e.type === "tool_result"}
              <pre data-selectable>{e.text}</pre>
            {:else if e.type === "notice" || e.type === "asked" || e.type === "reply"}
              <pre data-selectable>{e.text}</pre>
            {:else}
              <div class="empty">{i.block}</div>
            {/if}
          </div>
        {/if}
      </li>
    {:else}
      <li class="none">{t.items.length ? "Nothing matches these filters." : "Nothing was logged yet."}</li>
    {/each}
  </ol>
  {#if filtered.length > limit}
    <button type="button" class="more" onclick={() => (limit += 300)}>Show {Math.min(300, filtered.length - limit)} more</button>
  {/if}
</div>

<style>
  .timeline {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-2);
    font-size: var(--text-sm);
    min-width: 0;
  }
  .bar {
    display: flex;
    gap: var(--space-2);
    min-width: 0;
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
    min-width: 0;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
  }
  .clear {
    border: none;
    background: none;
    color: var(--text-faint);
    cursor: pointer;
  }
  select {
    height: 32px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
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
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-full);
    background: transparent;
    color: var(--text-faint);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .chip.on {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
    background: var(--accent-soft);
    color: var(--text);
  }
  .chip .n {
    color: var(--text-faint);
  }
  .cat {
    height: 26px;
    font-size: var(--text-xs);
  }
  .fail {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .spacer {
    flex: 1;
  }
  .links {
    white-space: nowrap;
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .locked {
    display: flex;
    gap: var(--space-2);
    align-items: flex-start;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warn) 12%, transparent);
    color: var(--text);
    font-size: var(--text-xs);
  }
  .count {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .rows {
    min-width: 0;
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .row {
    min-width: 0;
    margin-left: calc(var(--depth) * 18px);
    border-radius: var(--radius-md);
    border-left: 2px solid transparent;
  }
  .row.open {
    background: var(--surface-2);
    border-left-color: var(--border-strong);
  }
  .row.failed {
    border-left-color: var(--danger);
  }
  .row-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    min-width: 0;
    padding: 5px var(--space-2);
    border: none;
    border-radius: var(--radius-md);
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row-head:hover {
    background: var(--surface-hover, var(--surface-2));
  }
  .time {
    flex: none;
    width: 58px;
    color: var(--text-faint);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 3px;
  }
  .title {
    flex: none;
    max-width: 30%;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind,
  .path {
    flex: none;
    color: var(--text-faint);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .path {
    font-family: var(--font-mono);
  }
  .preview {
    flex: 1;
    min-width: 0;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ms {
    flex: none;
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .badge {
    flex: none;
    padding: 0 6px;
    border-radius: var(--radius-full);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
  }
  .badge.failed,
  .badge.stopped {
    background: color-mix(in srgb, var(--danger) 14%, transparent);
    color: var(--danger);
  }
  .badge.caught {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .detail {
    display: grid;
    gap: var(--space-2);
    padding: 0 var(--space-3) var(--space-3) calc(58px + var(--space-4));
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-3);
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .mono {
    font-family: var(--font-mono);
  }
  .sect {
    margin-top: var(--space-1);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 650;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .val {
    display: grid;
    gap: 3px;
  }
  .val-head {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
  }
  .port {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .val-meta {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  pre {
    margin: 0;
    max-height: 360px;
    overflow: auto;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.55;
    white-space: pre-wrap;
    word-break: break-word;
  }
  pre.reasoning {
    color: var(--text-muted);
    font-style: italic;
  }
  .error {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    white-space: pre-wrap;
  }
  .empty,
  .none {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .none {
    padding: var(--space-3);
    text-align: center;
  }
  .sent summary {
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
    margin-bottom: var(--space-1);
  }
  .more {
    justify-self: center;
    padding: var(--space-1) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
    color: var(--text-muted);
    cursor: pointer;
  }
  .compact .title {
    max-width: 40%;
  }
  .compact .kind {
    display: none;
  }
</style>
