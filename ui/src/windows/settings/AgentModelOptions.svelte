<script lang="ts">
  /**
   * An agent's model settings: for each of the model connection's settings
   * (temperature, longest answer, reasoning effort, sampling…), inherit the
   * model's (the default), set the agent's own, or don't send it at all.
   * Applies to every model call the agent makes; a model block's own
   * setting in the loop still wins.
   */
  import { untrack } from "svelte";
  import { commands, errorText, type Agent, type ProviderView } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { describe, EFFORTS, PARAMS, parseParam, showParam, unsupported, type ModelOverrides, type ParamInfo } from "$lib/generation";
  import { kindLabel } from "$lib/format";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let { agent }: { agent: Agent } = $props();

  type Key = keyof ModelOverrides;
  type Row = Omit<ParamInfo, "key"> & { key: Key; required?: boolean };
  const ROWS: Row[] = [
    { key: "temperature", label: "Temperature", type: "float", min: 0, max: 2, placeholder: "e.g. 0.3", help: "Lower is more predictable." },
    {
      key: "max_tokens",
      label: "Longest answer (tokens)",
      type: "int",
      min: 16,
      max: 32768,
      placeholder: "e.g. 2048",
      help: "Used by model blocks that don't set their own.",
      required: true,
    },
    ...PARAMS,
  ];
  type Mode = "inherit" | "set" | "off";

  const model = $derived<ProviderView | undefined>(
    settings.current.providers.find((p) => p.id === (agent.model ?? settings.current.default_provider)),
  );
  const fromModel = (k: Key): unknown => {
    const m = model;
    if (!m) return undefined;
    if (k === "temperature") return m.temperature ?? null;
    if (k === "max_tokens") return m.max_tokens;
    return (m.generation ?? {})[k as keyof typeof m.generation] ?? null;
  };

  const modes = $state<Record<string, Mode>>({});
  const texts = $state<Record<string, string>>({});
  let error = $state("");
  let notice = $state("");
  let busy = $state(false);
  let saved = $state("{}");

  function load(o: ModelOverrides) {
    for (const r of ROWS) {
      const has = Object.prototype.hasOwnProperty.call(o, r.key);
      const v = o[r.key];
      const unset = v === null || (Array.isArray(v) && !v.length);
      modes[r.key] = !has ? "inherit" : unset ? "off" : "set";
      texts[r.key] = has && !unset ? showParam(r.type, v) : "";
    }
    saved = JSON.stringify(o);
  }
  // Reload when another agent is shown, not on every settings save.
  $effect(() => {
    void agent.id;
    untrack(() => load(agent.model_options ?? {}));
  });

  const parsed = $derived(
    ROWS.map((r) => {
      const mode = modes[r.key] ?? "inherit";
      const res = mode === "set" ? parseParam(r, texts[r.key] ?? "") : null;
      const missing = mode === "set" && res && "value" in res && (res.value === null || (Array.isArray(res.value) && !res.value.length));
      return { r, mode, res, missing };
    }),
  );
  const options = $derived.by(() => {
    const o: Record<string, unknown> = {};
    for (const { r, mode, res } of parsed) {
      if (mode === "off") o[r.key] = r.type === "stop" ? [] : null;
      else if (mode === "set" && res && "value" in res && res.value !== null) o[r.key] = res.value;
    }
    return o as ModelOverrides;
  });
  const bad = $derived(parsed.some(({ res, missing }) => (res && "error" in res) || missing));
  const dirty = $derived(JSON.stringify(options) !== saved);
  const changed = $derived(parsed.filter(({ mode }) => mode !== "inherit").length);

  async function save() {
    busy = true;
    error = "";
    notice = "";
    try {
      await commands.setAgentModelOptions(agent.id, options);
      saved = JSON.stringify(options);
      notice = "Saved. The agent's next model call uses these.";
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
  function resetAll() {
    for (const r of ROWS) {
      modes[r.key] = "inherit";
      texts[r.key] = "";
    }
  }
  function setMode(k: Key, m: Mode) {
    modes[k] = m;
    // Start from the model's value, to edit it.
    if (m === "set" && !(texts[k] ?? "").trim()) {
      const row = ROWS.find((r) => r.key === k)!;
      const v = fromModel(k);
      texts[k] = v === null || v === undefined ? "" : showParam(row.type, v);
    }
  }
</script>

<div class="card card-pad">
  <div class="section-title"><Icon name="cpu" size={16} /> Model settings for {agent.name}</div>
  <p class="explain">
    {#if model}
      Every setting starts from {model.name} ({kindLabel[model.kind]}, Settings → Models). Change any for this agent only, or
      choose <b>Don't send</b> to leave it to the server. A model block's own setting in the loop still wins.
    {:else}
      This agent has no model connection yet; pick one above.
    {/if}
  </p>

  <div class="rows">
    {#each parsed as { r, mode, res, missing } (r.key)}
      {@const why = model && r.key !== "temperature" && r.key !== "max_tokens" ? unsupported(model.kind, r.key as never) : null}
      <div class="row" class:changed={mode !== "inherit"} class:off={!!why}>
        <div class="name">
          <span class="row-title">{r.label}</span>
          <span class="row-sub">
            {#if why}{why}{:else}Model: {describe(r.type, fromModel(r.key))}{/if}
          </span>
        </div>
        <select class="input mode" value={mode} onchange={(e) => setMode(r.key, e.currentTarget.value as Mode)} aria-label={`${r.label}: inherit, set or don't send`}>
          <option value="inherit">Inherit</option>
          <option value="set">Set</option>
          {#if !r.required}<option value="off">Don't send</option>{/if}
        </select>
        <div class="value">
          {#if mode === "set"}
            {#if r.type === "effort"}
              <select class="input" bind:value={texts[r.key]} aria-label={r.label}>
                <option value="" disabled>Choose…</option>
                {#each EFFORTS as e (e.value)}<option value={e.value}>{e.label}</option>{/each}
              </select>
            {:else if r.type === "stop"}
              <textarea class="input mono" rows="2" bind:value={texts[r.key]} placeholder="One per line" aria-label={r.label} spellcheck="false"></textarea>
            {:else}
              <input
                class="input"
                bind:value={texts[r.key]}
                inputmode={r.type === "int" ? "numeric" : "decimal"}
                placeholder={r.placeholder}
                aria-label={r.label}
                aria-invalid={(res && "error" in res) || missing}
              />
            {/if}
            {#if res && "error" in res}<span class="err">{res.error}</span>{:else if missing}<span class="err">Enter a value, or choose Inherit.</span>{/if}
          {:else if mode === "off"}
            <span class="muted small">Not sent: the server's default.</span>
          {:else}
            <span class="muted small" title={r.help}>{r.help}</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
  {#if notice && !dirty}<div class="notice"><Icon name="check" size={16} /><span>{notice}</span></div>{/if}

  <div class="actions">
    <span class="muted small">{changed ? `${changed} changed for this agent` : "All inherited from the model"}</span>
    <span class="spacer"></span>
    <Button variant="ghost" onclick={resetAll} disabled={busy || !changed}>Inherit all</Button>
    <Button variant="primary" onclick={save} disabled={busy || bad || !dirty}>Save</Button>
  </div>
</div>

<style>
  .section-title {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 600;
  }
  .explain {
    margin: var(--space-2) 0 var(--space-4);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .rows {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(150px, 1.1fr) 136px minmax(160px, 1.6fr);
    gap: var(--space-3);
    align-items: start;
    padding: var(--space-2) 0;
    border-top: 1px solid var(--border);
  }
  .name {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .row-title {
    font-size: var(--text-sm);
  }
  .changed .row-title {
    font-weight: 600;
  }
  .row-sub {
    color: var(--text-muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .off .row-title {
    color: var(--text-faint);
  }
  .value {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .value .muted {
    padding-top: 6px;
  }
  .err {
    color: var(--danger);
    font-size: var(--text-xs);
  }
  textarea {
    resize: vertical;
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    margin-top: var(--space-4);
  }
  .spacer {
    flex: 1;
  }
  .notice {
    margin-top: var(--space-3);
  }
</style>
