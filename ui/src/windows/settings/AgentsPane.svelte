<script lang="ts" module>
  import type { AgentTool, Overrides } from "$lib/ipc";
  import type { ModelOverrides } from "$lib/generation";

  /** A template agent left for another pane (to add a model or set up
   *  tools), picked up again when Agents opens. */
  interface Stash {
    name: string;
    loopId: string;
    model: string;
    wsToken: string | null;
    wsPath: string;
    tools: Record<string, AgentTool>;
    modelOptions: ModelOverrides;
    blocks: Overrides;
  }
  let stash: Stash | null = null;
</script>

<script lang="ts">
  import { onDestroy } from "svelte";
  import { commands, errorText, type Agent } from "$lib/ipc";
  import { date } from "$lib/format";
  import { settings } from "$lib/stores/settings.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import AgentAccess from "./AgentAccess.svelte";
  import AgentLoopConfig from "./AgentLoopConfig.svelte";
  import AgentModelOptions from "./AgentModelOptions.svelte";

  let {
    gotoModels,
    gotoTools,
    gotoSecurity,
  }: { gotoModels: () => void; gotoTools: () => void; gotoSecurity: () => void } = $props();

  // Carry on with a template agent left for another pane, if there is one.
  const kept = stash;
  stash = null;
  let editing = $state<Agent | "new" | null>(kept ? "new" : null);
  let name = $state(kept?.name ?? "");
  let loopId = $state(kept?.loopId ?? "");
  let model = $state(kept?.model ?? "");
  let wsToken = $state<string | null>(kept?.wsToken ?? null);
  let wsPath = $state(kept?.wsPath ?? "");
  let error = $state("");
  let saving = $state(false);
  // A template agent's settings, kept here until "Create agent" saves them.
  let tplTools = $state<Record<string, AgentTool>>(kept?.tools ?? {});
  let tplModel = $state<ModelOverrides>(kept?.modelOptions ?? {});
  let tplBlocks = $state<Overrides>(kept?.blocks ?? {});
  onDestroy(() => {
    if (editing === "new") {
      stash = {
        name,
        loopId,
        model,
        wsToken,
        wsPath,
        tools: $state.snapshot(tplTools),
        modelOptions: $state.snapshot(tplModel),
        blocks: $state.snapshot(tplBlocks),
      };
    }
  });

  const agents = $derived(settings.current.agents);
  /** The agent being edited, kept current as its settings change. */
  const live = $derived.by(() => {
    const e = editing;
    return e && e !== "new" ? (agents.find((a) => a.id === e.id) ?? null) : null;
  });
  /** The agent being created, as the sections below show it. */
  const template = $derived<Agent>({
    id: "",
    name,
    loop_id: loopId,
    model: model || null,
    workspace: { path: wsPath, confirmed_at: 0 },
    created_at: 0,
    tools: tplTools,
    model_options: tplModel,
  });
  const providers = $derived(settings.current.providers);
  const defaultModel = $derived(
    providers.find((p) => p.id === settings.current.default_provider) ?? providers[0] ?? null,
  );

  function open(a: Agent | "new") {
    editing = a;
    error = "";
    wsToken = null;
    tplTools = {};
    tplModel = {};
    tplBlocks = {};
    if (a === "new") {
      name = agents.length ? "" : "Assistant";
      // The recommended loop, if it's in the library.
      loopId = (loops.list.find((l) => l.name === "Chummy assistant (medium)") ?? loops.list[0])?.id ?? "";
      // Nothing set up yet: start with the recommended loop.
      if (!loops.list.length) {
        commands
          .addRecommendedLoop()
          .then((id) => {
            if (editing === "new" && !loopId) loopId = id;
          })
          .catch(() => {});
      }
      model = "";
      wsPath = "";
    } else {
      name = a.name;
      loopId = a.loop_id;
      model = a.model ?? "";
      wsPath = a.workspace.path;
    }
  }

  async function pick() {
    error = "";
    const picked = await commands.pickWorkspace(editing && editing !== "new" ? editing.id : null);
    if (picked) {
      wsToken = picked.token;
      wsPath = picked.path;
    }
  }

  async function save() {
    saving = true;
    error = "";
    let created = "";
    try {
      const id = editing && editing !== "new" ? editing.id : "";
      created = await commands.saveAgent({ id, name, loop_id: loopId, model: model || null }, wsToken);
      if (!id) {
        // A template agent: now that it exists, save the rest. Tools ask
        // for confirmation where they must.
        if (Object.keys(tplTools).length) await commands.setAgentAccess(created, tplTools);
        if (Object.keys(tplModel).length) await commands.setAgentModelOptions(created, tplModel);
        for (const [key, o] of Object.entries(tplBlocks)) await commands.setAgentBlockConfig(created, key, o);
      }
      editing = null;
    } catch (e) {
      error = errorText(e);
      // Created, but not everything saved: carry on editing the real agent.
      const made = created && settings.current.agents.find((a) => a.id === created);
      if (made && editing === "new") {
        editing = made;
        wsToken = null;
      }
    } finally {
      saving = false;
    }
  }

  function modelName(id: string | null) {
    if (!id) return defaultModel ? `Default model (${defaultModel.name})` : "Default model";
    return providers.find((p) => p.id === id)?.name ?? "Missing model, uses the default";
  }
</script>

{#if editing}
  <button class="back" type="button" onclick={() => (editing = null)}><Icon name="chevron-left" size={16} />Agents</button>
  <h1>{editing === "new" ? "New agent" : `Edit ${editing.name}`}</h1>
  <p class="pane-sub">An agent runs one loop with one model, inside its own workspace folder.</p>
  {#if editing === "new"}
    <p class="pane-sub">Set everything up here, then create it. New agents start with no tools; switch on what this one needs below.</p>
  {/if}

  <div class="card card-pad">
    <div class="field">
      <label for="ag-name">Name</label>
      <input id="ag-name" class="input" bind:value={name} placeholder="e.g. Researcher" />
    </div>

    <div class="field">
      <label for="ag-loop">Loop</label>
      {#if loops.list.length}
        <div class="input-row">
          <select id="ag-loop" class="input" bind:value={loopId}>
            {#each loops.list as l (l.id)}<option value={l.id}>{l.name}{!l.trusted ? " (needs a review)" : l.ok ? "" : " (has problems)"}</option>{/each}
          </select>
          {#if loops.list.find((l) => l.id === loopId)?.trusted === false}
            <Button icon="shield" onclick={() => commands.reviewLibraryLoop(loopId).catch((e) => (error = errorText(e)))}>Review loop</Button>
          {:else}
            <Button icon="flow" onclick={() => commands.openWindow("editor", loopId || null)}>Edit loop</Button>
          {/if}
        </div>
        {#if loops.list.find((l) => l.id === loopId)?.trusted === false}
          <span class="hint warn-text">This loop isn't on your list of trusted loops, so agents can't run it until you review it.</span>
        {:else}
          <span class="hint">How the agent thinks. Build and change loops in the loop editor.</span>
        {/if}
      {:else}
        <div class="notice">
          <Icon name="flow" size={16} />
          <span style="flex: 1">There are no loops yet. Add the recommended one, or build your own.</span>
          <Button variant="primary" onclick={async () => { try { loopId = await commands.addRecommendedLoop(); } catch (e) { error = errorText(e); } }}>Use recommended</Button>
          <Button variant="ghost" onclick={() => commands.openWindow("editor")}>Loop editor</Button>
        </div>
      {/if}
    </div>

    <div class="field">
      <label for="ag-model">Model</label>
      {#if providers.length}
        <select id="ag-model" class="input" bind:value={model}>
          <option value="">{modelName(null)}</option>
          {#each providers as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
        </select>
        <span class="hint">Used by every “Ask the model” block unless the block picks its own.</span>
      {:else}
        <div class="notice">
          <Icon name="bot" size={16} />
          <span style="flex: 1">No models are connected yet.</span>
          <Button onclick={gotoModels}>Add a model</Button>
        </div>
      {/if}
    </div>

    <div class="field">
      <span class="label">Workspace folder</span>
      <div class="ws">
        <span class="ws-icon"><Icon name="folder" size={18} /></span>
        <div class="ws-path">
          {#if wsPath}
            <div class="mono" data-selectable>{wsPath}</div>
            <div class="faint xs">{wsToken ? "Confirmed. Saved when you save the agent." : "This agent's folder."}</div>
          {:else}
            <div class="muted small">No folder chosen yet.</div>
          {/if}
        </div>
        <Button icon="folder" variant={wsPath ? "secondary" : "primary"} onclick={pick}>{wsPath ? "Change…" : "Choose…"}</Button>
      </div>
      <span class="hint">
        The only folder this agent can see. Best kept to this agent alone; sharing it with other agents is allowed after a warning.
        Whole drives, your home folder, system and hidden settings folders, and Chummy's own folder are never allowed.
      </span>
    </div>

    {#if editing !== "new"}
      {@render actions()}
    {/if}
  </div>

  {#snippet actions()}
    {#if error}<div class="notice danger" style="margin-top: var(--space-4)"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
    <div class="actions">
      {#if editing === "new" && !wsPath}<span class="muted small">Choose a workspace folder to create the agent.</span>{/if}
      <span class="spacer"></span>
      <Button variant="ghost" onclick={() => (editing = null)}>Cancel</Button>
      <Button variant="primary" onclick={save} disabled={saving || !name.trim() || !loopId || !wsPath}>
        {editing === "new" ? "Create agent" : "Save"}
      </Button>
    </div>
  {/snippet}

  {#if editing === "new"}
    <AgentAccess agent={template} {gotoTools} {gotoSecurity} template bind:value={tplTools} />
    <AgentModelOptions agent={template} template bind:value={tplModel} />
    {#if loopId}<AgentLoopConfig agent={template} template bind:value={tplBlocks} />{/if}
    <div class="card card-pad create">{@render actions()}</div>
  {:else if live}
    <AgentAccess agent={live} {gotoTools} {gotoSecurity} />
    <AgentModelOptions agent={live} />
    {#if loopId !== live.loop_id}
      <div class="card card-pad">
        <div class="notice"><Icon name="info" size={16} /><span>Save the agent to configure its new loop's blocks.</span></div>
      </div>
    {:else}
      <AgentLoopConfig agent={live} />
    {/if}
  {/if}
{:else}
  <header class="pane-head">
    <div>
      <h1>Agents</h1>
      <p class="pane-sub">Each agent has a loop, a model, and a workspace folder, the only one it can see.</p>
    </div>
    {#if agents.length}<Button variant="primary" icon="plus" onclick={() => open("new")}>New agent</Button>{/if}
  </header>

  {#if !agents.length}
    <div class="card empty">
      <EmptyState icon="bot" title="No agents yet">
        Set one up to start chatting: pick a loop, a model and a folder for it to work in.
        {#snippet actions()}
          <Button variant="primary" icon="plus" onclick={() => open("new")}>New agent</Button>
        {/snippet}
      </EmptyState>
    </div>
  {/if}

  {#each agents as a (a.id)}
    {@const loopName = loops.name(a.loop_id)}
    {@const n = Object.keys(a.tools ?? {}).length}
    <div class="card card-pad agent">
      <div class="a-head">
        <div class="a-main">
          <div class="a-name">
            {a.name}
            {#if settings.current.active_agent === a.id}<span class="badge accent">In chat</span>{/if}
          </div>
          <div class="muted small">
            {#if loopName}{loopName}{#if loops.list.find((l) => l.id === a.loop_id)?.trusted === false} <span class="warn-text">(needs a review)</span>{/if}{:else}<span class="warn-text">Loop missing</span>{/if}
            · {modelName(a.model)}
          </div>
          <div class="faint xs mono a-ws" title={a.workspace.path}>{a.workspace.path}</div>
          <div class="faint xs">
            {n ? `${n} tool${n === 1 ? "" : "s"}` : "No tools"}
            · {Object.values(a.tools ?? {}).some((t) => t.network) ? "internet allowed" : "offline"}
            {#if Object.keys(a.loop_config?.[a.loop_id] ?? {}).length}· custom block settings{/if}
          </div>
        </div>
        <div class="a-actions">
          {#if settings.current.active_agent !== a.id}
            <Button icon="chat" onclick={() => commands.setActiveAgent(a.id)}>Chat</Button>
          {/if}
          <Button variant="ghost" icon="edit" iconOnly aria-label="Edit" onclick={() => open(a)} />
          <Button variant="ghost" icon="trash" iconOnly aria-label="Delete" onclick={() => commands.deleteAgent(a.id)} />
        </div>
      </div>
      <div class="faint xs" style="margin-top: 4px">Created {date(a.created_at)}</div>
    </div>
  {/each}
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
  .ws {
    display: flex;
    gap: var(--space-3);
    align-items: center;
    padding: var(--space-3);
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: var(--surface-2);
  }
  .ws-icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 34px;
    height: 34px;
    border-radius: var(--radius-md);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .ws-path {
    flex: 1;
    min-width: 0;
    word-break: break-all;
  }
  .a-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .a-main {
    flex: 1;
    min-width: 0;
  }
  .a-name {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 600;
  }
  .a-ws {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-top: 2px;
  }
  .a-actions {
    display: flex;
    gap: 2px;
    align-items: center;
  }
  .warn-text {
    color: var(--warn);
  }
  .create .actions {
    margin-top: 0;
  }
</style>
