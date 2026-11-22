<script lang="ts">
  import { commands, type Block, type BlockPorts, type FunctionDef, type Issue, type LoopFile, type ParamSpec } from "$lib/ipc";
  import { loops } from "$lib/stores/loops.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { tools } from "$lib/stores/tools.svelte";
  import { outputCandidates, portKey } from "./loopdoc";
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
    scope = null,
    innerPorts = {},
    uses = 0,
    onfunctionchange,
    onopenfunction,
    ondeletefunction,
  }: {
    block: Block | null;
    meta: LoopFile;
    issues: Issue[];
    onchange: (b: Block) => void;
    ondelete: (id: string) => void;
    onselect: (id: string, fn: string | null) => void;
    /** The function being edited (null: the loop itself). */
    scope?: FunctionDef | null;
    /** Ports of the blocks inside `scope`. */
    innerPorts?: Record<string, BlockPorts>;
    /** How many function blocks use `scope`. */
    uses?: number;
    onfunctionchange: (f: FunctionDef) => void;
    onopenfunction: (id: string) => void;
    ondeletefunction: (id: string) => void;
  } = $props();

  const fnOf = (b: Block | null) =>
    b?.type === "function" ? (meta.functions ?? []).find((f) => f.id === b.params.function) : undefined;
  const blockFn = $derived(fnOf(block));
  const roleOptions = ["user", "assistant", "system", "tool"];

  function roles(): string[] {
    const v = block?.params.roles;
    return Array.isArray(v) ? (v as string[]) : ["user"];
  }
  function toolList(): string[] {
    const v = block?.params.tools;
    return Array.isArray(v) ? (v as string[]) : [];
  }
  function toggleTool(name: string, on: boolean) {
    const cur = toolList().filter((t) => t !== name);
    setParam("tools", on ? [...cur, name] : cur.length ? cur : undefined);
  }

  function setFn(patch: Partial<FunctionDef>) {
    if (scope) onfunctionchange({ ...scope, ...patch });
  }
  const candidates = $derived(scope ? outputCandidates(scope, innerPorts) : []);
  function blockLabel(id: string) {
    const b = scope?.blocks.find((x) => x.id === id);
    return b?.label || id;
  }
  function portLabel(id: string, port: string, dir: "inputs" | "outputs") {
    return innerPorts[id]?.[dir].find((p) => p.name === port)?.label ?? port;
  }
  function setOutput(block: string, port: string, on: boolean) {
    if (!scope) return;
    const k = portKey(block, port);
    let outputs = scope.outputs.filter((o) => portKey(o.from.block, o.from.port) !== k);
    if (on) {
      const taken = new Set([...scope.inputs.map((i) => i.name), ...scope.outputs.map((o) => o.name)]);
      let n = 1;
      while (taken.has(`out${n}`)) n++;
      outputs = [...outputs, { name: `out${n}`, label: "", from: { block, port } }];
    }
    setFn({ outputs });
  }

  const info = $derived(block ? loops.info(block.type) : undefined);
  $effect(() => {
    if (info?.params.some((p) => p.kind === "tools")) tools.init();
  });
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
    {#if block.type === "function"}
      {#if blockFn}
        <div class="fn-card">
          <div class="row-title">{blockFn.name}</div>
          {#if blockFn.description}<p class="hint">{blockFn.description}</p>{/if}
          <p class="hint">{blockFn.inputs.length} input{blockFn.inputs.length === 1 ? "" : "s"} · {blockFn.outputs.length} output{blockFn.outputs.length === 1 ? "" : "s"} · {blockFn.blocks.length} blocks</p>
          <Button icon="layers" onclick={() => onopenfunction(blockFn.id)}>Open function</Button>
        </div>
      {:else}
        <p class="hint">This block's function is missing.</p>
      {/if}
    {/if}

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
          {:else if p.kind === "choice"}
            <select
              id="in-{p.key}"
              class="input"
              value={(value(p) as string) ?? p.options[0]?.value}
              onchange={(e) => setParam(p.key, (e.currentTarget as HTMLSelectElement).value)}
            >
              {#each p.options as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
            </select>
          {:else if p.kind === "roles"}
            <div class="roles">
              {#each roles() as r, i (i)}
                <div class="role">
                  <span class="faint xs">{i + 1}.</span>
                  <select class="input" value={r} onchange={(e) => { const v = [...roles()]; v[i] = (e.currentTarget as HTMLSelectElement).value; setParam(p.key, v); }}>
                    {#each roleOptions as o (o)}<option value={o}>{o[0].toUpperCase() + o.slice(1)}</option>{/each}
                  </select>
                  {#if roles().length > 1}
                    <button type="button" class="mini" aria-label="Remove" onclick={() => setParam(p.key, roles().filter((_, j) => j !== i))}><Icon name="minus" size={13} /></button>
                  {/if}
                </div>
              {/each}
              <button type="button" class="mini add" onclick={() => setParam(p.key, [...roles(), "assistant"])}><Icon name="plus" size={13} /> Add role</button>
            </div>
          {:else if p.kind === "tools"}
            {#if !tools.ready}
              <p class="hint">No tools yet. Set them up in Settings → Tools.</p>
              <Button variant="ghost" icon="wrench" onclick={() => commands.openWindow("settings", "tools")}>Open Tools settings</Button>
            {:else}
              <div class="tools">
                {#each tools.installed as t (t.name)}
                  <label class="check" title={t.description}>
                    <input type="checkbox" checked={toolList().includes(t.name)} onchange={(e) => toggleTool(t.name, (e.currentTarget as HTMLInputElement).checked)} />
                    <span class="tname">{t.title}</span>
                    {#if !t.enabled || !settings.current.permissions.tools}<span class="off">off</span>{/if}
                  </label>
                {/each}
                {#each toolList().filter((n) => !tools.installed.some((t) => t.name === n)) as missing (missing)}
                  <label class="check"><input type="checkbox" checked onchange={() => toggleTool(missing, false)} /><span class="tname mono">{missing}</span><span class="off">not installed</span></label>
                {/each}
              </div>
            {/if}
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
  {:else if scope}
    <div class="head"><Icon name="layers" size={15} /><span class="kind">Function</span></div>
    <p class="desc">A block made of these blocks. It waits for all its inputs, runs them, and sends out what reaches its outputs.</p>
    <div class="field">
      <label for="fn-name">Name</label>
      <input id="fn-name" class="input" value={scope.name} oninput={(e) => setFn({ name: (e.currentTarget as HTMLInputElement).value })} />
    </div>
    <div class="field">
      <label for="fn-desc">Description</label>
      <textarea id="fn-desc" class="input area plain" rows="2" value={scope.description} oninput={(e) => setFn({ description: (e.currentTarget as HTMLTextAreaElement).value })}></textarea>
    </div>
    <div class="section">Inputs</div>
    {#each scope.inputs as inp, i (inp.name)}
      <div class="port-row">
        <span class="faint xs">{blockLabel(inp.to.block)} · {portLabel(inp.to.block, inp.to.port, "inputs")}</span>
        <input class="input" placeholder="Label" value={inp.label} oninput={(e) => { const v = [...scope.inputs]; v[i] = { ...inp, label: (e.currentTarget as HTMLInputElement).value }; setFn({ inputs: v }); }} />
      </div>
    {:else}
      <p class="hint">None: it runs once each time the loop starts.</p>
    {/each}
    <span class="hint">Every input that no block here feeds becomes an input of the function.</span>
    <div class="section">Outputs</div>
    {#each candidates as c (portKey(c.block, c.port.name))}
      {@const o = scope.outputs.find((x) => x.from.block === c.block && x.from.port === c.port.name)}
      <div class="port-row">
        <label class="check">
          <input type="checkbox" checked={!!o} onchange={(e) => setOutput(c.block, c.port.name, (e.currentTarget as HTMLInputElement).checked)} />
          <span class="faint xs">{blockLabel(c.block)} · {c.port.label}</span>
        </label>
        {#if o}
          <input class="input" placeholder="Label" value={o.label} oninput={(e) => { const v = scope.outputs.map((x) => (x === o ? { ...o, label: (e.currentTarget as HTMLInputElement).value } : x)); setFn({ outputs: v }); }} />
        {/if}
      </div>
    {:else}
      <p class="hint">No open outputs.</p>
    {/each}
    <div class="section">Checks</div>
    {#if !issues.length}
      <p class="ok"><Icon name="check-circle" size={15} /> No problems found.</p>
    {:else}
      <ul class="issues">
        {#each [...errors, ...warnings] as i, n (n)}
          <li class={i.severity}>
            <Icon name={i.severity === "error" ? "x-circle" : "alert"} size={14} />
            {#if i.block}<button type="button" class="link" onclick={() => onselect(i.block!, scope.id)}>{i.message}</button>{:else}<span>{i.message}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
    <div class="foot">
      <span class="faint xs">Used by {uses} block{uses === 1 ? "" : "s"}</span>
      <Button variant="ghost" icon="trash" disabled={uses > 0} onclick={() => ondeletefunction(scope.id)}>Delete function</Button>
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
      <div>
        <label for="in-tools">Max tool calls</label>
        <input id="in-tools" class="input" type="number" min="1" max="1000" value={meta.limits.max_tool_calls ?? 20} oninput={(e) => (meta.limits = { ...meta.limits, max_tool_calls: Number((e.currentTarget as HTMLInputElement).value) || 1 })} />
      </div>
    </div>
    <span class="hint">A run stops when it reaches any limit, so a loop that cycles can't run forever. Blocks inside functions count too.</span>

    <div class="section">Checks</div>
    {#if !issues.length}
      <p class="ok"><Icon name="check-circle" size={15} /> No problems found.</p>
    {:else}
      <ul class="issues">
        {#each [...errors, ...warnings] as i, n (n)}
          <li class={i.severity}>
            <Icon name={i.severity === "error" ? "x-circle" : "alert"} size={14} />
            {#if i.block}
              <button type="button" class="link" onclick={() => onselect(i.block!, i.function ?? null)}>{i.function ? `${(meta.functions ?? []).find((f) => f.id === i.function)?.name ?? i.function}: ` : ""}{i.message}</button>
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
  .fn-card {
    display: grid;
    gap: var(--space-2);
    justify-items: start;
    margin-bottom: var(--space-4);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface-2);
  }
  .row-title {
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .roles,
  .tools {
    display: grid;
    gap: 4px;
  }
  .role {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .mini {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .mini.add {
    justify-self: start;
  }
  .tname {
    flex: 1;
  }
  .off {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .port-row {
    display: grid;
    gap: 3px;
    margin-bottom: var(--space-2);
  }
  .xs {
    font-size: var(--text-xs);
  }
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
