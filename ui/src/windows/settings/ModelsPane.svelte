<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, type Preset, type ProbeReport, type ProviderView } from "$lib/ipc";
  import { kindLabel, precisionLabel } from "$lib/format";
  import { settings } from "$lib/stores/settings.svelte";
  import { local } from "$lib/stores/local.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import ProbeResults from "./ProbeResults.svelte";
  import ProviderForm from "./ProviderForm.svelte";
  import LocalModelWizard from "./LocalModelWizard.svelte";

  let { view = $bindable("list"), gotoEngine }: { view: "list" | "pick" | "form" | "local"; gotoEngine: () => void } = $props();

  let presets = $state<Preset[]>([]);
  let chosen = $state<Preset | null>(null);
  let editing = $state<ProviderView | null>(null);
  let tests = $state<Record<string, { running: boolean; report: ProbeReport | null; error: string; needsRuntime: boolean }>>({});

  const providers = $derived(settings.current.providers);

  onMount(async () => {
    presets = await commands.listPresets();
  });

  function pick(p: Preset) {
    chosen = p;
    editing = null;
    view = "form";
  }

  function edit(p: ProviderView) {
    editing = p;
    chosen = null;
    view = "form";
  }

  async function test(p: ProviderView) {
    tests[p.id] = { running: true, report: null, error: "", needsRuntime: false };
    try {
      const out = await commands.testProvider(p, null);
      tests[p.id] = { running: false, report: out.report, error: "", needsRuntime: out.needs_runtime };
    } catch (e) {
      tests[p.id] = { running: false, report: null, error: errorText(e), needsRuntime: false };
    }
  }

  async function remove(p: ProviderView) {
    if (p.kind === "local" && p.local) {
      await commands.deleteLocalModel(p.local.dir_name);
    } else {
      await commands.deleteProvider(p.id);
    }
    delete tests[p.id];
  }

  function finished(id: string) {
    view = "list";
    const p = settings.current.providers.find((x) => x.id === id);
    if (p) test(p);
  }

  const localPresets = $derived(presets.filter((p) => p.local));
  const cloudPresets = $derived(presets.filter((p) => !p.local));
</script>

{#if view === "form"}
  <ProviderForm preset={chosen} existing={editing} ondone={finished} oncancel={() => (view = "list")} />
{:else if view === "local"}
  <LocalModelWizard oncancel={() => (view = "pick")} ondone={finished} />
{:else if view === "pick"}
  <button class="back" type="button" onclick={() => (view = "list")}><Icon name="chevron-left" size={16} />Models</button>
  <h1>Add a model</h1>
  <p class="pane-sub">Pick where the model runs. You can add as many as you like and switch between them.</p>

  <div class="section-label">On this computer or your own server</div>
  <div class="tiles">
    <button class="tile featured" type="button" onclick={() => (view = "local")}>
      <span class="tile-icon"><Icon name="cpu" size={18} /></span>
      <span class="tile-text">
        <span class="tile-title">Built-in engine <span class="badge">Basic</span></span>
        <span class="tile-sub">Download a model from Hugging Face and run it here. Simple, but slower than the options below.</span>
      </span>
    </button>
    {#each localPresets as p (p.id)}
      <button class="tile" type="button" onclick={() => pick(p)}>
        <span class="tile-icon"><Icon name="laptop" size={18} /></span>
        <span class="tile-text"><span class="tile-title">{p.label}</span><span class="tile-sub">{p.hint}</span></span>
      </button>
    {/each}
  </div>

  <div class="section-label">Cloud</div>
  <div class="tiles">
    {#each cloudPresets as p (p.id)}
      <button class="tile" type="button" onclick={() => pick(p)}>
        <span class="tile-icon"><Icon name="cloud" size={18} /></span>
        <span class="tile-text"><span class="tile-title">{p.label}</span>{#if p.hint}<span class="tile-sub">{p.hint}</span>{/if}</span>
      </button>
    {/each}
  </div>
{:else}
  <header class="pane-head">
    <div>
      <h1>Models</h1>
      <p class="pane-sub">The language models your agents can use. Test a connection to check it can call tools.</p>
    </div>
    {#if providers.length}<Button variant="primary" icon="plus" onclick={() => (view = "pick")}>Add a model</Button>{/if}
  </header>

  {#if !providers.length}
    <div class="card empty">
      <EmptyState icon="bot" title="No models connected">
        Run one on this computer (Ollama, LM Studio, llama.cpp, vLLM or the built-in engine) or use a cloud provider.
        {#snippet actions()}
          <Button variant="primary" icon="plus" onclick={() => (view = "pick")}>Add a model</Button>
        {/snippet}
      </EmptyState>
    </div>
  {/if}

  {#each providers as p (p.id)}
    {@const t = tests[p.id]}
    {@const isDefault = settings.current.default_provider === p.id}
    {@const dl = p.local ? local.downloads[p.local.repo] : undefined}
    <div class="card card-pad provider">
      <div class="p-head">
        <div class="p-main">
          <div class="p-name">
            {p.name}
            {#if isDefault}<span class="badge accent">Default</span>{/if}
            {#if p.kind === "local"}<span class="badge">Basic engine</span>{/if}
          </div>
          <div class="muted small mono p-meta">
            {kindLabel[p.kind]} · {p.model}
            {#if p.local} · {precisionLabel[p.local.precision]}{/if}
            {#if p.has_key} · key saved{/if}
          </div>
        </div>
        <div class="p-actions">
          <Button icon="play" onclick={() => test(p)} disabled={t?.running}>{t?.running ? "Testing…" : "Test"}</Button>
          {#if p.kind !== "local"}<Button variant="ghost" icon="edit" iconOnly aria-label="Edit" onclick={() => edit(p)} />{/if}
          {#if !isDefault}<Button variant="ghost" icon="star" iconOnly aria-label="Make default" title="Make default" onclick={() => commands.setDefaultProvider(p.id)} />{/if}
          <Button variant="ghost" icon="trash" iconOnly aria-label="Remove" onclick={() => remove(p)} />
        </div>
      </div>

      {#if dl && !dl.done && !dl.error}
        <div class="p-body muted small">Downloading…</div>
      {/if}
      {#if t?.running && p.kind === "local"}
        <div class="p-body muted small">
          {local.engine.state === "loading" ? "Loading the model into memory. The first time can take a few minutes…" : "Starting the engine…"}
        </div>
      {/if}
      {#if t?.needsRuntime}
        <div class="p-body notice">
          <Icon name="cpu" size={16} />
          <span style="flex: 1">The built-in engine needs a one-time setup (Python and PyTorch) before it can run this model.</span>
          <Button variant="primary" onclick={async () => { if (await local.startInstall()) gotoEngine(); }}>Set up</Button>
        </div>
      {/if}
      {#if t?.report}<div class="p-body"><ProbeResults report={t.report} /></div>{/if}
      {#if t?.error}<div class="p-body notice danger"><Icon name="x-circle" size={16} /><span class="pre-wrap">{t.error}</span></div>{/if}
    </div>
  {/each}

  <p class="faint xs" style="margin-top: var(--space-4)">
    Model connections are made by Chummy for you and aren't affected by the agent internet permission.
  </p>
{/if}

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .pane-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-4);
  }
  .empty {
    display: flex;
    border-style: dashed;
    border-color: var(--border-strong);
    background: transparent;
  }
  .p-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .p-main {
    flex: 1;
    min-width: 0;
  }
  .p-name {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 600;
  }
  .p-meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .p-actions {
    display: flex;
    gap: 2px;
    align-items: center;
  }
  .p-body {
    margin-top: var(--space-3);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border);
  }
  .p-body.notice {
    border-top: none;
    align-items: center;
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: var(--space-2);
  }
  .tile {
    display: flex;
    gap: var(--space-3);
    align-items: flex-start;
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    text-align: left;
    cursor: pointer;
    transition: border-color var(--dur-fast) var(--ease), background var(--dur-fast) var(--ease);
  }
  .tile:hover {
    border-color: var(--border-strong);
    background: var(--surface-hover);
  }
  .tile.featured {
    grid-column: 1 / -1;
  }
  .tile-icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: var(--radius-md);
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .featured .tile-icon {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .tile-text {
    display: grid;
    gap: 2px;
  }
  .tile-title {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 600;
  }
  .tile-sub {
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
</style>
