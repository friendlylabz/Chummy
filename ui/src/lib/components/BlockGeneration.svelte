<script lang="ts">
  /**
   * A model block's generation settings in one collapsible group: temperature,
   * longest answer (block params of their own) and reasoning effort, sampling,
   * penalties, seed and stop sequences (its `generation` object). Each one
   * inherits the model's setting unless it's set here.
   */
  import { settings } from "$lib/stores/settings.svelte";
  import { blockOverrides, blockParams, modelValue, type ModelOverrides, type SettingKey } from "$lib/generation";
  import Icon from "./Icon.svelte";
  import OverrideRows from "./OverrideRows.svelte";

  let {
    params,
    onset,
    idPrefix = "in",
  }: {
    params: Record<string, unknown>;
    /** `undefined` removes the parameter. */
    onset: (key: string, value: unknown) => void;
    idPrefix?: string;
  } = $props();

  const value = $derived(blockOverrides(params));
  const count = $derived(Object.keys(value).length);
  // The block's own model, else the default one (the agent isn't known here).
  const model = $derived(
    settings.current.providers.find((p) => p.id === ((params.model as string | undefined) || settings.current.default_provider)),
  );
  let open = $state(false);
  let opened = false;
  $effect(() => {
    // Open by itself once when something's set, so it isn't hidden.
    if (!opened && count) {
      opened = true;
      open = true;
    }
  });
  let bad = $state(false);
  const keys = ["temperature", "max_tokens", "generation"] as const;

  function change(o: ModelOverrides) {
    const next = blockParams(o);
    for (const k of keys) {
      if (JSON.stringify(next[k]) !== JSON.stringify(params[k] ?? undefined)) onset(k, next[k]);
    }
  }
  const NO_OFF: SettingKey[] = ["temperature", "max_tokens"];
</script>

<details class="gen" bind:open>
  <summary>
    <span class="chev"><Icon name="chevron-down" size={14} /></span>
    <span class="title">Generation settings</span>
    {#if bad}<span class="count bad">check the values</span>
    {:else}<span class="count" class:set={count > 0}>{count ? `${count} set here` : "all from the model"}</span>{/if}
  </summary>
  <p class="hint">
    For this block only. <b>Inherit</b> uses the model's setting{model ? ` (${model.name})` : ""} and the agent's changes to it;
    <b>Don't send</b> leaves it to the server.
  </p>
  <OverrideRows
    {value}
    onchange={change}
    bind:bad
    kind={model?.kind}
    inherited={(k) => modelValue(model, k)}
    from="Model"
    noOff={NO_OFF}
    compact
    {idPrefix}
  />
</details>

<style>
  .gen {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--space-2) var(--space-3);
    background: var(--surface);
  }
  summary {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    cursor: pointer;
    list-style: none;
    font-size: var(--text-sm);
  }
  summary::-webkit-details-marker {
    display: none;
  }
  .chev {
    display: inline-flex;
    color: var(--text-muted);
    transform: rotate(-90deg);
    transition: transform var(--dur-fast) var(--ease);
  }
  .gen[open] .chev {
    transform: none;
  }
  .title {
    font-weight: 600;
    white-space: nowrap;
  }
  .count {
    margin-left: auto;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .count.set {
    color: var(--accent);
    font-weight: 600;
  }
  .count.bad {
    color: var(--danger);
  }
  .hint {
    margin: var(--space-2) 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
</style>
