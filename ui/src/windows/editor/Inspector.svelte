<script lang="ts">
  import type { Block, Issue, LoopFile, ParamSpec } from "$lib/ipc";
  import { loops } from "$lib/stores/loops.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import { categoryColor, categoryLabel } from "./loopdoc";

  let {
    block,
    meta = $bindable(),
    issues,
    onchange,
    ondelete,
    onselect,
  }: {
    block: Block | null;
    meta: LoopFile;
    issues: Issue[];
    onchange: (b: Block) => void;
    ondelete: (id: string) => void;
    onselect: (id: string) => void;
  } = $props();

  const info = $derived(block ? loops.info(block.type) : undefined);
  const blockIssues = $derived(block ? issues.filter((i) => i.block === block.id) : []);
  const errors = $derived(issues.filter((i) => i.severity === "error"));
  const warnings = $derived(issues.filter((i) => i.severity === "warning"));

  function setParam(key: string, value: unknown) {
    if (!block) return;
    const params = { ...block.params };
    if (value === undefined || value === "") delete params[key];
    else params[key] = value;
    onchange({ ...block, params });
  }

  function value(p: ParamSpec): unknown {
    return block?.params[p.key];
  }

  function numberInput(p: ParamSpec, raw: string) {
    if (raw.trim() === "") return setParam(p.key, p.optional ? undefined : 0);
    const n = Number(raw);
    if (!isNaN(n)) setParam(p.key, p.kind === "integer" ? Math.round(n) : n);
  }
</script>

<aside class="inspector">
  {#if block && info}
    <div class="head">
      <span class="dot" style:background={categoryColor[info.category]}></span>
      <span class="kind">{info.label}</span>
      <span class="cat">{categoryLabel[info.category]}</span>
    </div>
    <p class="desc">{info.description}</p>

    <div class="field">
      <label for="in-label">Name</label>
      <input
        id="in-label"
        class="input"
        value={block.label}
        oninput={(e) => onchange({ ...block, label: (e.currentTarget as HTMLInputElement).value })}
      />
    </div>

    {#each info.params as p (p.key)}
      <div class="field">
        {#if p.kind === "bool"}
          <label class="check">
            <input type="checkbox" checked={!!value(p)} onchange={(e) => setParam(p.key, (e.currentTarget as HTMLInputElement).checked)} />
            {p.label}
          </label>
        {:else}
          <label for="in-{p.key}">{p.label}</label>
          {#if p.kind === "multiline"}
            <textarea
              id="in-{p.key}"
              class="input area"
              rows={p.key === "prompt" || p.key === "format" ? 8 : 4}
              spellcheck="false"
              value={(value(p) as string) ?? ""}
              oninput={(e) => setParam(p.key, (e.currentTarget as HTMLTextAreaElement).value)}
            ></textarea>
          {:else if p.kind === "text"}
            <input
              id="in-{p.key}"
              class="input mono"
              spellcheck="false"
              value={(value(p) as string) ?? ""}
              oninput={(e) => setParam(p.key, (e.currentTarget as HTMLInputElement).value)}
            />
          {:else if p.kind === "number" || p.kind === "integer"}
            <input
              id="in-{p.key}"
              class="input"
              type="number"
              min={p.min}
              max={p.max}
              step={p.kind === "number" ? p.step : 1}
              placeholder={p.optional ? "Default" : ""}
              value={value(p) ?? ""}
              oninput={(e) => numberInput(p, (e.currentTarget as HTMLInputElement).value)}
            />
          {:else if p.kind === "model"}
            <select
              id="in-{p.key}"
              class="input"
              value={(value(p) as string) ?? ""}
              onchange={(e) => setParam(p.key, (e.currentTarget as HTMLSelectElement).value || undefined)}
            >
              <option value="">The agent's model</option>
              {#each settings.current.providers as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
            </select>
          {/if}
        {/if}
        {#if p.help}<span class="hint">{p.help}</span>{/if}
      </div>
    {/each}

    {#if blockIssues.length}
      <ul class="issues">
        {#each blockIssues as i, n (n)}
          <li class={i.severity}><Icon name={i.severity === "error" ? "x-circle" : "alert"} size={14} /><span>{i.message}</span></li>
        {/each}
      </ul>
    {/if}

    <div class="foot">
      <span class="faint mono">{block.id}</span>
      <Button variant="ghost" icon="trash" onclick={() => ondelete(block.id)}>Delete block</Button>
    </div>
  {:else}
    <div class="head"><span class="kind">Loop</span></div>
    <div class="field">
      <label for="in-desc">Description</label>
      <textarea id="in-desc" class="input area plain" rows="3" bind:value={meta.description} placeholder="What this loop is for"></textarea>
    </div>
    <div class="field two">
      <div>
        <label for="in-steps">Max steps</label>
        <input id="in-steps" class="input" type="number" min="1" max="10000" bind:value={meta.limits.max_steps} />
      </div>
      <div>
        <label for="in-calls">Max model calls</label>
        <input id="in-calls" class="input" type="number" min="1" max="1000" bind:value={meta.limits.max_llm_calls} />
      </div>
    </div>
    <span class="hint">A run stops when it reaches either limit, so a loop that cycles can't run forever.</span>

    <div class="section">Checks</div>
    {#if !issues.length}
      <p class="ok"><Icon name="check-circle" size={15} /> No problems found.</p>
    {:else}
      <ul class="issues">
        {#each [...errors, ...warnings] as i, n (n)}
          <li class={i.severity}>
            <Icon name={i.severity === "error" ? "x-circle" : "alert"} size={14} />
            {#if i.block}
              <button type="button" class="link" onclick={() => onselect(i.block!)}>{i.message}</button>
            {:else}
              <span>{i.message}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    <p class="hint" style="margin-top: var(--space-4)">Select a block to edit it. Delete or Backspace removes the selection.</p>
  {/if}
</aside>

<style>
  .inspector {
    flex: none;
    width: 300px;
    overflow-y: auto;
    padding: var(--space-4);
    border-left: 1px solid var(--border);
    background: var(--surface);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 3px;
  }
  .kind {
    font-weight: 650;
  }
  .cat {
    margin-left: auto;
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .desc {
    margin: var(--space-2) 0 var(--space-4);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .field {
    display: grid;
    gap: 5px;
    margin-bottom: var(--space-4);
  }
  .field.two {
    grid-template-columns: 1fr 1fr;
    gap: var(--space-2);
    margin-bottom: var(--space-1);
  }
  .field.two > div {
    display: grid;
    gap: 5px;
  }
  label {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .check {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 400;
  }
  .hint {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .input {
    width: 100%;
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    outline: none;
    font-size: var(--text-sm);
  }
  .input:focus {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border-strong));
  }
  .input.mono,
  .area {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .area.plain {
    font-family: var(--font-sans);
    font-size: var(--text-sm);
  }
  .area {
    height: auto;
    padding: var(--space-2) var(--space-3);
    resize: vertical;
    line-height: 1.5;
  }
  .section {
    margin: var(--space-5) 0 var(--space-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .ok {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .ok :global(svg) {
    color: var(--ok);
  }
  .issues {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-2);
    font-size: var(--text-sm);
  }
  .issues li {
    display: flex;
    gap: var(--space-2);
    align-items: flex-start;
  }
  .issues li :global(svg) {
    flex: none;
    margin-top: 3px;
  }
  .issues .error :global(svg) {
    color: var(--danger);
  }
  .issues .warning :global(svg) {
    color: var(--warn);
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    text-align: left;
    color: inherit;
    font-size: inherit;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: var(--space-5);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border);
  }
  .faint {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .mono {
    font-family: var(--font-mono);
  }
</style>
