<script lang="ts">
  import { untrack } from "svelte";
  import { DEFAULT_DAILY_TOKENS, DEFAULT_MAX_TOKENS, commands, errorText, type Preset, type ProbeReport, type ProviderConfig, type ProviderView } from "$lib/ipc";
  import { kindLabel } from "$lib/format";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import ProbeResults from "./ProbeResults.svelte";

  let {
    preset = null,
    existing = null,
    ondone,
    oncancel,
  }: {
    preset?: Preset | null;
    existing?: ProviderView | null;
    ondone: (id: string) => void;
    oncancel: () => void;
  } = $props();

  // Captured once: the form edits a copy.
  const start = untrack(() => existing) ?? null;
  const p0 = untrack(() => preset) ?? null;
  let name = $state(start?.name ?? p0?.label ?? "");
  let baseUrl = $state(start?.base_url ?? p0?.base_url ?? "");
  let model = $state(start?.model ?? "");
  let temperature = $state(start?.temperature != null ? String(start.temperature) : "");
  let apiKey = $state("");
  let maxTokens = $state(String(start?.max_tokens ?? DEFAULT_MAX_TOKENS));
  const maxValue = $derived(Math.round(Number(maxTokens.replace(/[,_\s]/g, ""))));
  const maxBad = $derived(!Number.isFinite(maxValue) || maxValue < 16 || maxValue > 32768);
  const limit0 = start ? (start.daily_tokens ?? null) : DEFAULT_DAILY_TOKENS;
  let limitOn = $state(limit0 !== null);
  let limit = $state(String(limit0 ?? DEFAULT_DAILY_TOKENS));
  const limitValue = $derived(Math.round(Number(limit.replace(/[,_\s]/g, ""))));
  const limitBad = $derived(limitOn && (!Number.isFinite(limitValue) || limitValue < 1000));
  let removeKey = $state(false);
  const kind = start?.kind ?? p0?.kind ?? "open_ai_compatible";
  const needsKey = p0?.needs_key ?? (start ? start.has_key : false);
  const hint = p0?.hint ?? "";

  let models = $state<string[]>([]);
  let loadingModels = $state(false);
  let modelsError = $state("");
  let testing = $state(false);
  let report = $state<ProbeReport | null>(null);
  let error = $state("");
  let saving = $state(false);

  function draft(): ProviderConfig {
    const t = temperature.trim() === "" ? null : Number(temperature);
    return {
      id: start?.id ?? "",
      name: name.trim(),
      kind,
      base_url: baseUrl.trim(),
      model: model.trim(),
      local: start?.local ?? null,
      temperature: t != null && !isNaN(t) ? t : null,
      daily_tokens: limitOn ? limitValue : null,
      max_tokens: maxBad ? DEFAULT_MAX_TOKENS : maxValue,
    };
  }

  function keyArg(): string | null {
    if (removeKey) return "";
    return apiKey.trim() === "" ? null : apiKey.trim();
  }

  async function loadModels() {
    loadingModels = true;
    modelsError = "";
    try {
      models = await commands.listModels(draft(), keyArg() || null);
      if (!model && models.length) model = models[0];
      if (!models.length) modelsError = "The server didn't list any models; type the name instead.";
    } catch (e) {
      modelsError = errorText(e);
    } finally {
      loadingModels = false;
    }
  }

  async function test() {
    testing = true;
    report = null;
    error = "";
    try {
      const out = await commands.testProvider(draft(), keyArg() || null);
      report = out.report;
    } catch (e) {
      error = errorText(e);
    } finally {
      testing = false;
    }
  }

  async function save() {
    saving = true;
    error = "";
    try {
      const id = await commands.saveProvider(draft(), keyArg());
      ondone(id);
    } catch (e) {
      error = errorText(e);
    } finally {
      saving = false;
    }
  }
</script>

<button class="back" type="button" onclick={oncancel}><Icon name="chevron-left" size={16} />Models</button>
<h1>{start ? `Edit ${start.name}` : `Add ${p0?.label ?? "a model"}`}</h1>
<p class="pane-sub">{hint || kindLabel[kind]}</p>

<div class="card card-pad">
  <div class="field">
    <label for="pf-name">Name</label>
    <input id="pf-name" class="input" bind:value={name} placeholder="What to call this connection" />
  </div>

  <div class="field">
    <label for="pf-url">Server address</label>
    <input id="pf-url" class="input mono" bind:value={baseUrl} placeholder="http://localhost:8080/v1" spellcheck="false" />
  </div>

  <div class="field">
    <label for="pf-key">API key {needsKey ? "" : "(optional)"}</label>
    <input
      id="pf-key"
      class="input mono"
      type="password"
      autocomplete="off"
      bind:value={apiKey}
      disabled={removeKey}
      placeholder={start?.has_key ? "Saved. Leave blank to keep it." : needsKey ? "Paste your key" : "Only if your server needs one"}
    />
    <span class="hint">
      Stored in your system keychain, never in Chummy's files.
      {#if start?.has_key}
        <label class="inline"><input type="checkbox" bind:checked={removeKey} /> Remove the saved key</label>
      {/if}
    </span>
  </div>

  <div class="field">
    <label for="pf-model">Model</label>
    <div class="input-row">
      <input id="pf-model" class="input mono" bind:value={model} list="pf-models" placeholder="e.g. llama3.2:3b" spellcheck="false" />
      <Button icon="refresh" onclick={loadModels} disabled={loadingModels || !baseUrl.trim()}>
        {loadingModels ? "Loading…" : "Load list"}
      </Button>
    </div>
    <datalist id="pf-models">
      {#each models as m (m)}<option value={m}></option>{/each}
    </datalist>
    {#if modelsError}<span class="hint">{modelsError}</span>{:else if models.length}<span class="hint">{models.length === 1 ? "1 model" : `${models.length} models`} available. Start typing to filter.</span>{/if}
  </div>

  <div class="field">
    <label for="pf-temp">Temperature (optional)</label>
    <input id="pf-temp" class="input" style="width: 120px" bind:value={temperature} inputmode="decimal" placeholder="Default" />
    <span class="hint">Lower is more predictable. Small models often do best around 0.2–0.4.</span>
  </div>

  <div class="field">
    <label for="pf-max">Longest answer (tokens)</label>
    <input id="pf-max" class="input" style="width: 150px" bind:value={maxTokens} inputmode="numeric" aria-invalid={maxBad} />
    <span class="hint">
      {#if maxBad}16 to 32,768 tokens.{:else}Used by every model block that doesn't set its own (in the loop editor or
        Agent Loop Configuration). Small models often ramble: a lower limit keeps answers short.{/if}
    </span>
  </div>

  <div class="field">
    <span class="label">Daily token limit</span>
    <div class="input-row limit">
      <label class="inline first"><input type="checkbox" bind:checked={limitOn} /> Limit to</label>
      <input
        class="input"
        style="width: 150px"
        bind:value={limit}
        inputmode="numeric"
        disabled={!limitOn}
        aria-label="Tokens per day"
        aria-invalid={limitBad}
      />
      <span class="muted small">tokens a day</span>
    </div>
    <span class="hint">
      {#if limitBad}At least 1,000 tokens.
      {:else}Counts what every agent and job sends to and gets from this model, and resets at midnight. When it runs out,
        each model block waits, stops, or asks you for more, as set in the block. Turn it off for no limit.{/if}
    </span>
  </div>

  {#if report}
    <div class="results"><ProbeResults {report} /></div>
  {/if}
  {#if error}
    <div class="notice danger results"><Icon name="x-circle" size={16} /><span class="pre-wrap">{error}</span></div>
  {/if}

  <div class="actions">
    <Button icon="play" onclick={test} disabled={testing || !model.trim() || !baseUrl.trim()}>
      {testing ? "Testing…" : "Test connection"}
    </Button>
    <span class="spacer"></span>
    <Button variant="ghost" onclick={oncancel}>Cancel</Button>
    <Button variant="primary" onclick={save} disabled={saving || !name.trim() || !model.trim() || limitBad || maxBad}>
      {start ? "Save" : "Add model"}
    </Button>
  </div>
</div>

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
  }
  .results {
    margin-top: var(--space-5);
    padding-top: var(--space-4);
    border-top: 1px solid var(--border);
  }
  .limit {
    align-items: center;
  }
  .inline.first {
    margin-left: 0;
  }
  .inline {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    margin-left: var(--space-2);
    font-weight: 400;
    cursor: pointer;
  }
</style>
