<script lang="ts">
  /**
   * An agent's model settings: for each of the model connection's settings
   * (temperature, longest answer, reasoning effort, sampling…), inherit the
   * model's (the default), set the agent's own, or don't send it at all.
   * Applies to every model call the agent makes; a model block's own
   * setting in the loop still wins.
   *
   * For a template agent (one being created) nothing is saved here: the
   * changes are kept in `value` and saved when the agent is created.
   */
  import { commands, errorText, type Agent, type ProviderView } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { modelValue, type ModelOverrides } from "$lib/generation";
  import { kindLabel } from "$lib/format";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import OverrideRows from "$lib/components/OverrideRows.svelte";

  let {
    agent,
    template = false,
    value = $bindable({}),
  }: { agent: Agent; template?: boolean; value?: ModelOverrides } = $props();

  const model = $derived<ProviderView | undefined>(
    settings.current.providers.find((p) => p.id === (agent.model ?? settings.current.default_provider)),
  );
  const canon = (o: ModelOverrides) => JSON.stringify(Object.fromEntries(Object.entries(o ?? {}).sort(([a], [b]) => (a < b ? -1 : 1))));

  // What's being edited, and what's saved; reset when another agent is shown.
  let shownFor = $state("");
  let options = $state<ModelOverrides>({});
  let saved = $state("{}");
  let bad = $state(false);
  let error = $state("");
  let notice = $state("");
  let busy = $state(false);
  $effect.pre(() => {
    if (agent.id !== shownFor) {
      shownFor = agent.id;
      options = { ...((template ? value : agent.model_options) ?? {}) };
      saved = canon(options);
      notice = "";
      error = "";
    }
  });
  const dirty = $derived(!template && canon(options) !== saved);
  function edit(o: ModelOverrides) {
    options = o;
    if (template) value = o;
  }
  const changed = $derived(Object.keys(options).length);

  async function save() {
    busy = true;
    error = "";
    notice = "";
    try {
      await commands.setAgentModelOptions(agent.id, options);
      saved = canon(options);
      notice = "Saved. The agent's next model call uses these.";
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="card card-pad">
  <div class="section-title"><Icon name="cpu" size={16} /> Model settings for {agent.name.trim() || "this agent"}</div>
  <p class="explain">
    {#if model}
      Every setting starts from {model.name} ({kindLabel[model.kind]}, Settings → Models). Change any for this agent only, or
      choose <b>Don't send</b> to leave it to the server. A model block's own setting in the loop still wins.
    {:else}
      There's no model connection yet. Add one in Settings → Models, then pick it above.
    {/if}
  </p>

  {#key shownFor}
    <OverrideRows
      value={options}
      onchange={edit}
      bind:bad
      kind={model?.kind}
      inherited={(k) => modelValue(model, k)}
      from="Model"
      idPrefix={`agent-${agent.id}`}
    />
  {/key}

  {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
  {#if notice && !dirty}<div class="notice"><Icon name="check" size={16} /><span>{notice}</span></div>{/if}

  <div class="actions">
    <span class="muted small">{changed ? `${changed} changed for this agent` : "All inherited from the model"}</span>
    <span class="spacer"></span>
    <Button variant="ghost" onclick={() => edit({})} disabled={busy || !changed}>Inherit all</Button>
    {#if template}
      <span class="faint xs">Saved when you create the agent.</span>
    {:else}
      <Button variant="primary" onclick={save} disabled={busy || bad || !dirty}>Save</Button>
    {/if}
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
