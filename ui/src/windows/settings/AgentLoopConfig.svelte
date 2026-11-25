<script lang="ts">
  /**
   * Agent Loop Configuration: change any block's settings for one agent
   * without changing the loop file. Saved per agent and per loop; the loop's
   * own settings apply to everything not changed here.
   */
  import { onDestroy, untrack } from "svelte";
  import { commands, errorText, type Agent, type AgentLoopConfig, type Block, type Overrides } from "$lib/ipc";
  import { loops } from "$lib/stores/loops.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import ParamFields from "$lib/components/ParamFields.svelte";

  let { agent }: { agent: Agent } = $props();

  let config = $state<AgentLoopConfig | null>(null);
  let overrides = $state<Overrides>({});
  let error = $state("");
  let notice = $state("");
  let open = $state<string | null>(null);
  const errors = $state<Record<string, string>>({});

  async function reload() {
    error = "";
    try {
      config = await commands.agentLoopConfig(agent.id);
      overrides = config.overrides;
    } catch (e) {
      config = null;
      error = errorText(e);
    }
  }

  // Reload only when the agent or its loop changes, not on every settings save.
  const which = $derived(`${agent.id}\u0000${agent.loop_id}`);
  $effect(() => {
    void which;
    untrack(() => {
      loops.init();
      reload();
    });
  });

  interface Item {
    key: string;
    block: Block;
  }
  const groups = $derived.by(() => {
    if (!config) return [];
    const usable = (b: Block) => (loops.info(b.type)?.params.length ?? 0) > 0;
    const g: { title: string; items: Item[] }[] = [
      { title: "Loop", items: config.file.blocks.filter(usable).map((b) => ({ key: b.id, block: b })) },
    ];
    for (const f of config.file.functions ?? []) {
      const items = f.blocks.filter(usable).map((b) => ({ key: `${f.id}/${b.id}`, block: b }));
      if (items.length) g.push({ title: `Function: ${f.name}`, items });
    }
    return g.filter((x) => x.items.length);
  });
  const changedCount = $derived(Object.keys(overrides).length);

  /** The block's settings with this agent's changes applied. */
  function merged(it: Item): Record<string, unknown> {
    const p = { ...it.block.params };
    for (const [k, v] of Object.entries(overrides[it.key] ?? {})) {
      if (v === null) delete p[k];
      else p[k] = v;
    }
    return p;
  }

  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  onDestroy(() => timers.forEach((t) => clearTimeout(t)));

  function set(it: Item, key: string, value: unknown) {
    const base = it.block.params[key];
    const o = { ...(overrides[it.key] ?? {}) };
    const v = value === "" ? undefined : value;
    if (v === undefined ? base === undefined : same(v, base)) delete o[key];
    else o[key] = v === undefined ? null : v;
    write(it.key, o);
  }

  function reset(it: Item, key: string) {
    const o = { ...(overrides[it.key] ?? {}) };
    delete o[key];
    write(it.key, o);
  }

  /** Saves one block's changes, a moment after the last keystroke. */
  function write(key: string, o: Record<string, unknown>) {
    const next = { ...overrides };
    if (Object.keys(o).length) next[key] = o;
    else delete next[key];
    overrides = next;
    clearTimeout(timers.get(key));
    timers.set(
      key,
      setTimeout(async () => {
        try {
          await commands.setAgentBlockConfig(agent.id, key, Object.keys(o).length ? o : null);
          delete errors[key];
        } catch (e) {
          errors[key] = errorText(e);
        }
      }, 400),
    );
  }

  async function resetAll() {
    for (const key of Object.keys(overrides)) {
      clearTimeout(timers.get(key));
      try {
        await commands.setAgentBlockConfig(agent.id, key, null);
      } catch (e) {
        error = errorText(e);
      }
    }
    await reload();
  }

  async function exportConfig() {
    notice = "";
    try {
      if (await commands.exportAgentLoopConfig(agent.id)) notice = "Exported.";
    } catch (e) {
      error = errorText(e);
    }
  }
  // Imported settings that change the loop are reviewed first, in the
  // Review window; when they're accepted there, the agent's settings change.
  let reviewing = false;
  const saved = $derived(JSON.stringify(agent.loop_config?.[agent.loop_id] ?? {}));
  $effect(() => {
    void saved;
    untrack(() => {
      if (!reviewing) return;
      reviewing = false;
      notice = "Loaded the reviewed settings.";
      reload();
    });
  });

  async function importConfig() {
    notice = "";
    try {
      const n = await commands.importAgentLoopConfig(agent.id);
      if (n !== null) {
        notice = `Loaded settings for ${n} block${n === 1 ? "" : "s"}.`;
        await reload();
      } else {
        reviewing = true;
      }
    } catch (e) {
      error = errorText(e);
    }
  }
</script>

<div class="card card-pad">
  <div class="head">
    <div class="section-title"><Icon name="sliders" size={16} /> Agent Loop Configuration</div>
    <span class="spacer"></span>
    <Button variant="ghost" icon="import" onclick={importConfig}>Import</Button>
    <Button variant="ghost" icon="export" onclick={exportConfig}>Export</Button>
  </div>
  <p class="explain">
    Change any block's settings for {agent.name} alone. The loop file keeps its own settings for every other agent;
    anything you don't change here follows the loop. Export saves these changes as a file you can load into another agent
    that uses the same loop.
  </p>

  {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
  {#if notice}<div class="notice"><Icon name="check-circle" size={16} /><span>{notice}</span></div>{/if}

  {#if config}
    {#if config.missing.length}
      <div class="notice warn">
        <Icon name="alert" size={16} />
        <span>Some saved changes are for blocks the loop no longer has: {config.missing.join(", ")}. They are ignored.</span>
      </div>
    {/if}
    <div class="summary muted small">
      {config.file.name} · {changedCount ? `${changedCount} block${changedCount === 1 ? "" : "s"} changed` : "No changes: the loop's own settings apply"}
      {#if changedCount}<button type="button" class="link" onclick={resetAll}>Reset all</button>{/if}
    </div>

    {#each groups as g (g.title)}
      <div class="section-label">{g.title}</div>
      <div class="blocks">
        {#each g.items as it (it.key)}
          {@const info = loops.info(it.block.type)}
          {@const changed = Object.keys(overrides[it.key] ?? {})}
          <div class="blk" class:open={open === it.key}>
            <button type="button" class="blk-head" aria-expanded={open === it.key} onclick={() => (open = open === it.key ? null : it.key)}>
              <Icon name={open === it.key ? "chevron-down" : "chevron-left"} size={14} />
              <span class="blk-name">{it.block.label || info?.label || it.block.type}</span>
              <span class="faint xs">{info?.label}</span>
              {#if changed.length}<span class="badge accent">{changed.length} changed</span>{/if}
              {#if errors[it.key]}<span class="badge danger">not saved</span>{/if}
            </button>
            {#if it.block.comment}<div class="comment">{it.block.comment}</div>{/if}
            {#if open === it.key && info}
              <div class="blk-body">
                {#if errors[it.key]}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{errors[it.key]}</span></div>{/if}
                <ParamFields
                  {info}
                  params={merged(it)}
                  {changed}
                  agentTools={agent.tools ?? []}
                  idPrefix="alc-{it.key}"
                  onset={(k, v) => set(it, k, v)}
                  onreset={(k) => reset(it, k)}
                />
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/each}
  {/if}
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .spacer {
    flex: 1;
  }
  .section-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 650;
  }
  .explain {
    margin: var(--space-2) 0 var(--space-3);
    color: var(--text-muted);
    font-size: var(--text-sm);
    line-height: 1.55;
  }
  .summary {
    display: flex;
    gap: var(--space-3);
    align-items: center;
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .blocks {
    display: grid;
    gap: 6px;
  }
  .blk {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface-2);
  }
  .blk.open {
    background: var(--surface);
  }
  .blk-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border: none;
    background: none;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .blk-name {
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .comment {
    margin: -4px var(--space-3) var(--space-2) 34px;
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-style: italic;
  }
  .blk-body {
    padding: var(--space-2) var(--space-3) var(--space-1);
    border-top: 1px solid var(--border);
  }
  .notice {
    margin-bottom: var(--space-3);
  }
</style>
