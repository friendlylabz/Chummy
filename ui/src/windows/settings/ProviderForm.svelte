<script lang="ts">
  import { untrack } from "svelte";
  import { DEFAULT_COMPACT_PERCENT, DEFAULT_CONTEXT, DEFAULT_DAILY_TOKENS, DEFAULT_MAX_TOKENS, MIN_CONTEXT, commands, compactThreshold, contextWindow, errorText, type Preset, type ProbeReport, type ProviderConfig, type ProviderView } from "$lib/ipc";
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
  /** Plain http:// to somewhere that isn't this computer or the local network. */
  const unencrypted = $derived.by(() => {
    let u: URL;
    try {
      u = new URL(baseUrl.trim());
    } catch {
      return false;
    }
    if (u.protocol !== "http:") return false;
    const h = u.hostname.replace(/^\[|\]$/g, "").replace(/\.$/, "").toLowerCase();
    const local =
      h === "localhost" ||
      /\.(localhost|local|lan|home\.arpa|internal)$/.test(h) ||
      /^(127\.|10\.|192\.168\.|169\.254\.)/.test(h) ||
      /^172\.(1[6-9]|2\d|3[01])\./.test(h) ||
      h === "::1" ||
      /^f[cd]/.test(h) ||
      /^fe80:/.test(h) ||
      (!h.includes(".") && !h.includes(":"));
    return !local;
  });
  let maxTokens = $state(String(start?.max_tokens ?? DEFAULT_MAX_TOKENS));
  const maxValue = $derived(Math.round(Number(maxTokens.replace(/[,_\s]/g, ""))));
  const maxBad = $derived(!Number.isFinite(maxValue) || maxValue < 16 || maxValue > 32768);
  const limit0 = start ? (start.daily_tokens ?? null) : DEFAULT_DAILY_TOKENS;
  let limitOn = $state(limit0 !== null);
  let limit = $state(String(limit0 ?? DEFAULT_DAILY_TOKENS));
  const limitValue = $derived(Math.round(Number(limit.replace(/[,_\s]/g, ""))));
  const limitBad = $derived(limitOn && (!Number.isFinite(limitValue) || limitValue < 1000));
  // Context: what the server reports (filled in by Test connection too), the
  // user's own length (never more), and when loops make room.
  let contextMax = $state<number | null>(start?.context_max ?? null);
  let contextTokens = $state(start?.context_tokens != null ? String(start.context_tokens) : "");
  let compactAt = $state(start?.compact_at != null ? String(start.compact_at) : "");
  const num = (t: string) => (t.trim() === "" ? null : Math.round(Number(t.replace(/[,_\s]/g, ""))));
  const contextValue = $derived(num(contextTokens));
  const compactValue = $derived(num(compactAt));
  const contextBad = $derived(
    contextValue !== null && (!Number.isFinite(contextValue) || contextValue < MIN_CONTEXT || (contextMax !== null && contextValue > contextMax)),
  );
  const compactBad = $derived(compactValue !== null && (!Number.isFinite(compactValue) || compactValue < MIN_CONTEXT));
  const effective = $derived({
    context_max: contextMax,
    context_tokens: contextBad ? null : contextValue,
    compact_at: compactBad ? null : compactValue,
  });
  const windowNow = $derived(contextWindow(effective));
  const compactNow = $derived(compactThreshold(effective));
  const fmt = (n: number) => n.toLocaleString("en-US");
  let useInApp = $state(start?.use_in_app ?? false);
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
      context_max: contextMax,
      context_tokens: contextBad ? null : contextValue,
      compact_at: compactBad ? null : compactValue,
      use_in_app: useInApp,
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
      if (report?.context) contextMax = report.context;
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
    {#if unencrypted}
      <span class="hint warn-hint">
        <Icon name="alert" size={13} /> Plain http:// off your local network isn't encrypted: your API key and conversations
        travel as they are, unless a VPN or similar protects them. Chummy asks you to confirm when you save.
      </span>
    {/if}
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
    <label for="pf-ctx">Context length (tokens)</label>
    <input
      id="pf-ctx"
      class="input"
      style="width: 150px"
      bind:value={contextTokens}
      inputmode="numeric"
      placeholder={fmt(contextMax ?? DEFAULT_CONTEXT)}
      aria-invalid={contextBad}
    />
    <span class="hint">
      {#if contextBad}{contextMax !== null ? `${fmt(MIN_CONTEXT)} to ${fmt(contextMax)} tokens (the most the server offers).` : `At least ${fmt(MIN_CONTEXT)} tokens.`}
      {:else if contextMax !== null}The server reports {fmt(contextMax)} tokens. Leave empty to use all of it, or set less if
        your server runs the model with a shorter context.
      {:else}The server doesn't say (OpenAI's own API never does; Test connection asks again). Leave empty to assume
        {fmt(DEFAULT_CONTEXT)} tokens, or set the length your server runs the model with.{/if}
    </span>
  </div>

  <div class="field">
    <label for="pf-compact">Make room at (tokens)</label>
    <input
      id="pf-compact"
      class="input"
      style="width: 150px"
      bind:value={compactAt}
      inputmode="numeric"
      placeholder={fmt(Math.floor((windowNow * DEFAULT_COMPACT_PERCENT) / 100))}
      aria-invalid={compactBad}
    />
    <span class="hint">
      {#if compactBad}At least {fmt(MIN_CONTEXT)} tokens.
      {:else}When a conversation reaches about {fmt(compactNow)} tokens, loops that check (like the Chummy assistant's
        Make room) summarise it so it fits. Empty means {DEFAULT_COMPACT_PERCENT}% of the context; it's never more than the
        context length.{/if}
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

  <div class="field">
    <label class="inline first"><input type="checkbox" bind:checked={useInApp} /> Use model within app</label>
    <span class="hint">
      Off: this model only answers your agents' loops. On: Chummy also uses it for small jobs of its own outside the
      loops, like naming chats from their first reply. Those calls count toward the daily limit.
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
    <Button variant="primary" onclick={save} disabled={saving || !name.trim() || !model.trim() || limitBad || maxBad || contextBad || compactBad}>
      {start ? "Save" : "Add model"}
    </Button>
  </div>
</div>

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
  }
  .warn-hint {
    display: flex;
    gap: 5px;
    align-items: flex-start;
    color: var(--warn);
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
