<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, events, type FlagLevel, type ReviewText, type ReviewView } from "$lib/ipc";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import SideNav from "$lib/components/SideNav.svelte";
  import FlowTree from "./FlowTree.svelte";
  import { blockName, originLabel } from "./names";

  type Tab = "flags" | "can" | "prompts" | "flows" | "texts" | "changes";

  let view = $state<ReviewView | null>(null);
  let error = $state("");
  let tab = $state<Tab>("flags");
  let understood = $state(false);
  let busy = $state(false);
  let main = $state<HTMLElement | null>(null);
  // Each tab starts at the top.
  $effect(() => {
    void tab;
    if (main) main.scrollTop = 0;
  });
  /** Capabilities that deserve attention when a loop has them. */
  const risky = new Set(["calls_without_model", "internet", "writes_files"]);

  async function show(token: string) {
    if (!token) {
      error = "There's nothing to review.";
      return;
    }
    try {
      view = await commands.getReview(token);
      error = "";
      tab = "flags";
      understood = false;
    } catch (e) {
      error = errorText(e);
    }
  }

  onMount(() => {
    show(location.hash.replace(/^#/, ""));
    events.onGoto(show);
  });

  const r = $derived(view?.review);
  const serious = $derived(r?.flags.filter((f) => f.level === "serious").length ?? 0);
  const warnings = $derived(r?.flags.filter((f) => f.level === "warning").length ?? 0);
  const notes = $derived(r?.flags.filter((f) => f.level === "note").length ?? 0);
  const isConfig = $derived(view?.source.kind === "loop_config");

  const tabs = $derived.by(() => {
    const t: { id: Tab; label: string; icon: "alert" | "shield" | "chat" | "flow" | "text" | "sliders" }[] = [
      { id: "flags", label: `Flags (${r?.flags.length ?? 0})`, icon: "alert" },
      { id: "can", label: "What it can do", icon: "shield" },
      { id: "prompts", label: `Prompts (${r?.models.length ?? 0})`, icon: "chat" },
      { id: "flows", label: "Where data flows", icon: "flow" },
      { id: "texts", label: "All its text", icon: "text" },
    ];
    if (isConfig) t.unshift({ id: "changes", label: "What changes", icon: "sliders" });
    return t;
  });

  const source = $derived.by(() => {
    const s = view?.source;
    if (!s) return "";
    if (s.kind === "import") return `From the file ${s.file_name}. It isn't on your list of trusted loops.`;
    if (s.kind === "library") return "In your loops, but not on your list of trusted loops (it may have been copied into Chummy's folder by hand), so it can't run or be edited yet.";
    return `Settings imported for ${s.agent_name}'s loop. They change its blocks, so here is the loop as ${s.agent_name} would run it.`;
  });

  const accept = $derived(
    view?.source.kind === "import" ? "Import and trust…" : view?.source.kind === "library" ? "Trust this loop…" : "Use these settings…",
  );
  const decline = $derived(view?.source.kind === "import" ? "Don't import" : view?.source.kind === "library" ? "Keep it untrusted" : "Don't use them");
  const canAccept = $derived(!!view && !busy && (serious === 0 || understood));

  async function yes() {
    if (!view) return;
    busy = true;
    error = "";
    try {
      await commands.acceptReview(view.token);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  async function no() {
    if (view) await commands.discardReview(view.token).catch(() => {});
  }

  const levelLabel: Record<FlagLevel, string> = { serious: "Serious", warning: "Warning", note: "Note" };

  function textOf(i: number): ReviewText | undefined {
    return r?.texts[i];
  }
  function where(t: ReviewText): string {
    return t.block === null || !r ? t.what : `${t.what} of ${blockName(r.blocks[t.block])}`;
  }
  function modelName(id: string): string {
    const m = view?.models.find((x) => x.id === id);
    return m?.name ? `${m.name} (${m.base_url ?? id})` : `“${id}”, which isn't one of your connections`;
  }

  /** Texts the flags point at, to mark them in the All its text tab. */
  const flagged = $derived(new Set(r?.flags.filter((f) => f.level !== "note").flatMap((f) => f.texts) ?? []));
</script>

{#snippet textBox(t: ReviewText, i: number)}
  <div class="textbox" class:flagged={flagged.has(i)} id={`text-${i}`}>
    <div class="texthead">
      <span>{where(t)}</span>
      {#if t.hidden.length}<span class="chip serious">{t.hidden.length} hidden character{t.hidden.length === 1 ? "" : "s"}</span>{/if}
      {#if t.urls.length}<span class="chip warning">web address</span>{/if}
      {#if !t.flows}<span class="chip quiet">name or note</span>{/if}
    </div>
    <pre data-selectable>{t.text}</pre>
  </div>
{/snippet}

<div class="window">
  {#if !view}
    <div class="empty">
      {#if error}<p>{error}</p>{:else}<p>Reading the loop…</p>{/if}
    </div>
  {:else if r}
    <header class="top">
      <div class="title">
        <span class="kicker">Review before trusting</span>
        <h1 data-selectable>{r.name}</h1>
        <p class="sub">{source}</p>
      </div>
      <div class="verdict" class:bad={serious > 0} class:meh={serious === 0 && warnings > 0}>
        <Icon name={serious ? "x-circle" : warnings ? "alert" : "info"} size={20} />
        <div>
          {#if serious}
            <strong>{serious} serious flag{serious === 1 ? "" : "s"}</strong>
          {:else if warnings}
            <strong>{warnings} warning{warnings === 1 ? "" : "s"}</strong>
          {:else}
            <strong>No serious flags or warnings</strong>
          {/if}
          <span>{serious && warnings ? `and ${warnings} warning${warnings === 1 ? "" : "s"}` : ""}{notes ? ` · ${notes} note${notes === 1 ? "" : "s"}` : ""}</span>
        </div>
      </div>
    </header>

    <section class="caution">
      <Icon name="shield" size={18} />
      <div>
        <p>
          <strong>Only use loops from people you trust.</strong> A loop is a program: it runs with everything its agent may do. Chummy read this one
          without running it and looked for risky patterns, but that can't prove it's safe. A clean review isn't a guarantee, and harmless loops can
          raise flags too.
        </p>
        <details>
          <summary>What a harmful loop could do</summary>
          <ul>
            <li>Read files in the agent's folder and send them to a website through a web tool, with or without a model involved.</li>
            <li>Change or delete files in the agent's folder, or plant a script or settings file that runs later, when you or another program open it.</li>
            <li>Talk a model into misusing its tools, with instructions in its prompts that look harmless or are hidden.</li>
            <li>Ask you for a password, key or code and pass on what you type.</li>
            <li>Send what the agent sees to a different model from the one you chose for it, say a cloud model instead of a local one.</li>
            <li>Behave well at first and differently later: after a few runs, or only for some messages, or when a web page it reads changes.</li>
            <li>Use up your daily tokens.</li>
          </ul>
          <p class="cant">
            What it can't do: leave the agent's folder, turn on a tool or internet access that the app or the agent has off, run anything outside a
            sandbox, or skip the approvals you've set for tools.
          </p>
        </details>
      </div>
    </section>

    <div class="body">
      <aside>
        <SideNav items={tabs} bind:active={tab} label="Review sections" />
        <dl class="facts">
          <dt>Blocks</dt>
          <dd>{r.block_count}{r.function_count ? ` in ${r.function_count + 1} parts` : ""}</dd>
          <dt>Limits per run</dt>
          <dd>{r.max_llm_calls} model calls, {r.max_tool_calls} tool calls</dd>
          <dt>Fingerprint</dt>
          <dd class="print" data-selectable title={r.fingerprint}>{r.fingerprint.slice(0, 16)}…</dd>
        </dl>
      </aside>

      <main bind:this={main}>
        {#if !r.complete}
          <p class="notice">This loop is too large to check completely, so some flows may be missing.</p>
        {/if}

        {#if tab === "changes" && view.source.kind === "loop_config"}
          <h2>What the imported settings change</h2>
          <p class="lead">Each row is one setting of one block. The other tabs show the loop with all of them applied.</p>
          <table>
            <thead><tr><th>Block</th><th>Setting</th><th>Now</th><th>Imported</th></tr></thead>
            <tbody>
              {#each view.source.changes as c (c.block + c.param)}
                <tr>
                  <td>{c.label}</td>
                  <td>{c.param}</td>
                  <td><pre data-selectable>{c.from ?? "(default)"}</pre></td>
                  <td><pre data-selectable>{c.to ?? "(default)"}</pre></td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else if tab === "flags"}
          <h2>Flags</h2>
          <p class="lead">
            Patterns Chummy looks for because they're risky, or because a harmless loop rarely needs them. Serious flags are worth stopping for; notes are
            just good to know.
          </p>
          {#if !r.flags.length}
            <p class="none">Nothing was flagged. Look through the other tabs anyway: the check can miss things.</p>
          {/if}
          {#each r.flags as f, i (i)}
            <article class="flag {f.level}">
              <div class="flaghead">
                <span class="chip {f.level}">{levelLabel[f.level]}</span>
                <h3>{f.title}</h3>
              </div>
              <p>{f.detail}</p>
              {#if f.blocks.length}
                <div class="blocks">
                  {#each f.blocks as b (b)}<span class="blockchip">{blockName(r.blocks[b])}</span>{/each}
                </div>
              {/if}
              {#each f.texts.slice(0, 6) as ti (ti)}
                {@const t = textOf(ti)}
                {#if t}{@render textBox(t, ti)}{/if}
              {/each}
              {#if f.texts.length > 6}<p class="more">…and {f.texts.length - 6} more texts (see All its text).</p>{/if}
            </article>
          {/each}
        {:else if tab === "can"}
          <h2>What it can do</h2>
          <p class="lead">What its blocks allow, worked out from how they're connected. The app's and the agent's switches still decide what actually runs.</p>
          <ul class="caps">
            {#each r.capabilities as c (c.key)}
              <li class:on={c.active} class:risky={c.active && risky.has(c.key)}>
                <Icon name={c.active ? (risky.has(c.key) ? "alert" : "check-circle") : "minus"} size={18} />
                <div><strong>{c.title}</strong><span>{c.detail}</span></div>
              </li>
            {/each}
          </ul>
          {#if r.description}
            <h3 class="sm">Its description (written by whoever made it)</h3>
            <pre class="desc" data-selectable>{r.description}</pre>
          {/if}
        {:else if tab === "prompts"}
          <h2>Prompts</h2>
          <p class="lead">
            Every model block, with the text it sends. <code>_INPUT_</code> is where the block's input goes, <code>{"{{name}}"}</code> reads memory and
            <code>_TOOLS_</code> lists the tools. Text from other blocks can reach the model too; it's listed under each one.
          </p>
          {#if !r.models.length}<p class="none">This loop has no model blocks.</p>{/if}
          {#each r.models as m (m.block)}
            <article class="model">
              <h3>{blockName(r.blocks[m.block])}</h3>
              <div class="chips">
                <span class="chip quiet">Sees: {m.sees.map(originLabel).join(", ") || "nothing"}</span>
                <span class="chip" class:warning={m.calls_tools}>{m.calls_tools ? `Can call ${m.tools}` : "Calls no tools"}</span>
                {#if m.online}<span class="chip warning">Tools may go online</span>{/if}
                {#if m.model}<span class="chip warning">Uses {modelName(m.model)}</span>{/if}
              </div>
              {#if m.system.trim()}<h4>System message</h4><pre data-selectable>{m.system}</pre>{/if}
              <h4>Prompt</h4>
              <pre data-selectable>{m.prompt || "_INPUT_"}</pre>
              {#if m.reminder.trim()}<h4>Reminder</h4><pre data-selectable>{m.reminder}</pre>{/if}
              {#if m.memory.length}<p class="small">Reads memory: {m.memory.join(", ")}</p>{/if}
              {#if m.texts.length}
                <details>
                  <summary>{m.texts.length} other text{m.texts.length === 1 ? "" : "s"} from this loop can reach this model</summary>
                  {#each m.texts as ti (ti)}
                    {@const t = textOf(ti)}
                    {#if t}{@render textBox(t, ti)}{/if}
                  {/each}
                </details>
              {/if}
            </article>
          {/each}
        {:else if tab === "flows"}
          <h2>Where data flows</h2>
          <p class="lead">
            Each block that acts (runs tools, writes or reads a file, calls a model, asks you, replies), with everything that can feed it, step by step
            backwards. Open a step to see what feeds it. The chips say where its text can come from.
          </p>
          {#each r.flows as f (f.step)}
            <FlowTree review={r} step={f.step} title={f.title} />
          {/each}
        {:else if tab === "texts"}
          <h2>All its text</h2>
          <p class="lead">
            Everything written into the loop file: prompts, fixed texts, formats, questions, names and notes. Hidden characters are shown as [U+…].
            Texts the flags point at are marked.
          </p>
          {#each r.texts as t, i (i)}{@render textBox(t, i)}{/each}
        {/if}
      </main>
    </div>

    <footer>
      {#if error}<p class="err">{error}</p>{/if}
      {#if serious}
        <label class="ack">
          <input type="checkbox" bind:checked={understood} />
          I've read the serious flags, and I trust where this {isConfig ? "configuration" : "loop"} came from.
        </label>
      {/if}
      <div class="buttons">
        <Button onclick={no}>{decline}</Button>
        <Button variant="primary" disabled={!canAccept} onclick={yes}>{accept}</Button>
      </div>
    </footer>
  {/if}
</div>

<style>
  .window {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .empty {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--text-muted);
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-5);
    padding: var(--space-5) var(--space-6) var(--space-3);
  }
  .kicker {
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .sub {
    margin-top: var(--space-1);
    color: var(--text-muted);
    max-width: 70ch;
  }
  .verdict {
    flex: none;
    display: flex;
    gap: var(--space-3);
    align-items: center;
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    color: var(--text-muted);
  }
  .verdict div {
    display: grid;
  }
  .verdict strong {
    color: var(--text);
  }
  .verdict span {
    font-size: var(--text-sm);
  }
  .verdict.bad {
    border-color: color-mix(in srgb, var(--danger) 45%, transparent);
    background: color-mix(in srgb, var(--danger) 8%, var(--surface));
    color: var(--danger);
  }
  .verdict.meh {
    border-color: color-mix(in srgb, var(--warn) 45%, transparent);
    color: var(--warn);
  }
  .caution {
    display: flex;
    gap: var(--space-3);
    margin: 0 var(--space-6) var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--warn) 10%, var(--surface));
    border: 1px solid color-mix(in srgb, var(--warn) 30%, transparent);
    color: var(--text);
    font-size: var(--text-sm);
  }
  .caution :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--warn);
  }
  .caution summary {
    margin-top: var(--space-2);
    cursor: pointer;
    font-weight: 600;
  }
  .caution ul {
    margin: var(--space-2) 0;
    padding-left: var(--space-5);
  }
  .cant {
    color: var(--text-muted);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    border-top: 1px solid var(--border);
  }
  aside {
    width: 230px;
    flex: none;
    border-right: 1px solid var(--border);
    overflow-y: auto;
  }
  .facts {
    margin: var(--space-3) var(--space-4);
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
  .facts dt {
    margin-top: var(--space-2);
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-weight: 600;
  }
  .facts dd {
    margin: 0;
  }
  .print {
    font-family: var(--font-mono);
  }
  main {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: var(--space-5) var(--space-6) var(--space-6);
  }
  h2 {
    font-size: var(--text-lg);
    font-weight: 650;
  }
  .lead {
    margin: var(--space-1) 0 var(--space-4);
    color: var(--text-muted);
    max-width: 75ch;
  }
  .none {
    color: var(--text-muted);
  }
  .notice {
    margin-bottom: var(--space-4);
    color: var(--warn);
  }
  article {
    margin-bottom: var(--space-4);
    padding: var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  article.serious {
    border-left: 3px solid var(--danger);
  }
  article.warning {
    border-left: 3px solid var(--warn);
  }
  .flaghead {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    margin-bottom: var(--space-2);
  }
  h3 {
    font-size: var(--text-md);
    font-weight: 650;
  }
  h3.sm {
    margin-top: var(--space-5);
    font-size: var(--text-sm);
  }
  h4 {
    margin: var(--space-3) 0 var(--space-1);
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text-faint);
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  article p {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .chip {
    display: inline-block;
    padding: 1px var(--space-2);
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 600;
    white-space: nowrap;
  }
  .chip.serious {
    background: color-mix(in srgb, var(--danger) 15%, transparent);
    color: var(--danger);
  }
  .chip.warning {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .chip.note,
  .chip.quiet {
    background: var(--surface-2);
    color: var(--text-muted);
    font-weight: 500;
  }
  .chips,
  .blocks {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin: var(--space-2) 0;
  }
  .blockchip {
    padding: 2px var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
  }
  .more,
  .small {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
  pre {
    margin: 0;
    padding: var(--space-2) var(--space-3);
    max-height: 260px;
    overflow: auto;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    word-break: break-word;
  }
  pre.desc {
    max-width: 75ch;
  }
  .textbox {
    margin-top: var(--space-2);
  }
  .textbox.flagged pre {
    outline: 1px solid color-mix(in srgb, var(--warn) 55%, transparent);
  }
  .texthead {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
    margin-bottom: 2px;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  details summary {
    margin-top: var(--space-3);
    cursor: pointer;
    font-size: var(--text-sm);
    color: var(--text-muted);
  }
  .caps {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .caps li {
    display: flex;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    color: var(--text-faint);
  }
  .caps li + li {
    border-top: 1px solid var(--border);
  }
  .caps li.on {
    color: var(--accent);
  }
  .caps li.risky {
    color: var(--warn);
  }
  .caps div {
    display: grid;
  }
  .caps strong {
    color: var(--text);
    font-weight: 600;
  }
  .caps li:not(.on) strong {
    color: var(--text-muted);
  }
  .caps span {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  th {
    text-align: left;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    padding: var(--space-2);
  }
  td {
    vertical-align: top;
    padding: var(--space-2);
    border-top: 1px solid var(--border);
  }
  code {
    font-family: var(--font-mono);
    font-size: 0.92em;
  }
  footer {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-6);
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .ack {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-size: var(--text-sm);
  }
  .buttons {
    margin-left: auto;
    display: flex;
    gap: var(--space-2);
  }
  .err {
    color: var(--danger);
    font-size: var(--text-sm);
  }
</style>
