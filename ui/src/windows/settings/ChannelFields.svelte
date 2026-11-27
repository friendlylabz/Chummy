<script lang="ts">
  /**
   * The fields a kind of channel asks, drawn from what the kind says
   * (Rust's `ChannelKind::fields`): the same in Settings → Channels and
   * Configure chummyd, for every kind. Secrets are typed here and never
   * shown again: a saved one shows as "Saved. Leave blank to keep it."
   */
  import { commands, errorText, type ChannelKindInfo } from "$lib/ipc";
  import Button from "$lib/components/Button.svelte";
  import Toggle from "$lib/components/Toggle.svelte";

  let {
    kind,
    options = $bindable(),
    secrets = $bindable(),
    saved = [],
    prefix = "ch",
  }: {
    kind: ChannelKindInfo;
    options: Record<string, unknown>;
    secrets: Record<string, string>;
    /** Secret fields with something saved. */
    saved?: string[];
    /** Keeps ids apart when several forms show. */
    prefix?: string;
  } = $props();

  /** What a check of an address said, by field. */
  let probed = $state<Record<string, { ok: boolean; text: string }>>({});
  let probing = $state<Record<string, boolean>>({});

  const text = (k: string) => (typeof options[k] === "string" ? (options[k] as string) : "");
  const lines = (k: string) => {
    const v = options[k];
    return Array.isArray(v) ? v.join("\n") : typeof v === "string" ? v : "";
  };

  async function probe(field: string) {
    probing[field] = true;
    delete probed[field];
    try {
      probed[field] = { ok: true, text: await commands.channelProbe(kind.id, field, text(field)) };
    } catch (e) {
      probed[field] = { ok: false, text: errorText(e) };
    } finally {
      delete probing[field];
    }
  }
</script>

{#each kind.fields as f (f.key)}
  {@const id = `${prefix}-${f.key}`}
  {#if f.input === "toggle"}
    <div class="toggle-field">
      <div>
        <div class="label">{f.label}</div>
        {#if f.hint}<div class="hint">{f.hint}</div>{/if}
      </div>
      <Toggle checked={options[f.key] === true} label={f.label} onchange={(v) => (options[f.key] = v)} />
    </div>
  {:else}
    <div class="field">
      <label for={id}>{f.label}</label>
      {#if f.input === "secret"}
        <input
          {id}
          class="input mono"
          type="password"
          autocomplete="off"
          spellcheck="false"
          value={secrets[f.key] ?? ""}
          oninput={(e) => (secrets[f.key] = e.currentTarget.value)}
          placeholder={saved.includes(f.key) ? "Saved. Leave blank to keep it." : f.placeholder}
        />
      {:else if f.input === "lines"}
        <textarea
          {id}
          class="input mono"
          rows="3"
          spellcheck="false"
          value={lines(f.key)}
          oninput={(e) => (options[f.key] = e.currentTarget.value)}
          placeholder={f.placeholder}
        ></textarea>
      {:else if f.input === "number"}
        <input
          {id}
          class="input narrow"
          type="number"
          min={f.min}
          max={f.max}
          value={typeof options[f.key] === "number" ? options[f.key] : ""}
          oninput={(e) => (options[f.key] = e.currentTarget.valueAsNumber)}
        />
      {:else}
        <div class="with-check">
          <input
            {id}
            class="input"
            class:mono={f.input === "url"}
            spellcheck="false"
            value={text(f.key)}
            oninput={(e) => {
              options[f.key] = e.currentTarget.value;
              delete probed[f.key];
            }}
            placeholder={f.placeholder}
          />
          {#if f.input === "url" && f.probe}
            <Button variant="ghost" onclick={() => probe(f.key)} disabled={probing[f.key] || !text(f.key)}>Check</Button>
          {/if}
        </div>
        {#if probed[f.key]}
          <span class="xs" class:ok={probed[f.key].ok} class:bad={!probed[f.key].ok}>{probed[f.key].text}</span>
        {/if}
      {/if}
      {#if f.hint}<span class="hint">{f.hint}</span>{/if}
    </div>
  {/if}
{/each}

<style>
  .toggle-field {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    margin: var(--space-4) 0;
  }
  .toggle-field > div {
    flex: 1;
  }
  .toggle-field .label {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .toggle-field .hint {
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .field {
    margin-bottom: var(--space-4);
  }
  .with-check {
    display: flex;
    gap: var(--space-2);
  }
  textarea.input {
    height: auto;
    padding: var(--space-2) var(--space-3);
    resize: vertical;
  }
  .narrow {
    width: 120px;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--danger);
  }
</style>
