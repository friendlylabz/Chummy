<script lang="ts">
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

  let { gotoModels, gotoTools }: { gotoModels: () => void; gotoTools: () => void } = $props();

  let editing = $state<Agent | "new" | null>(null);
  let name = $state("");
  let loopId = $state("");
  let model = $state("");
  let wsToken = $state<string | null>(null);
  let wsPath = $state("");
  let error = $state("");
  let saving = $state(false);

  const agents = $derived(settings.current.agents);
  /** The agent being edited, kept current as its settings change. */
  const live = $derived.by(() => {
    const e = editing;
    return e && e !== "new" ? (agents.find((a) => a.id === e.id) ?? null) : null;
  });
  const providers = $derived(settings.current.providers);
  const defaultModel = $derived(
    providers.find((p) => p.id === settings.current.default_provider) ?? providers[0] ?? null,
  );

  function open(a: Agent | "new") {
    editing = a;
    error = "";
    wsToken = null;
    if (a === "new") {
      name = agents.length ? "" : "Assistant";
      // The recommended loop, if it's in the library.
      loopId = (loops.list.find((l) => l.name === "Chummy assistant") ?? loops.list[0])?.id ?? "";
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
    try {
      const id = editing && editing !== "new" ? editing.id : "";
      await commands.saveAgent({ id, name, loop_id: loopId, model: model || null }, wsToken);
      editing = null;
    } catch (e) {
      error = errorText(e);
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
    <p class="pane-sub">New agents start with no tools and no internet. You can turn them on here after creating it.</p>
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

    {#if error}<div class="notice danger" style="margin-top: var(--space-4)"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}

    <div class="actions">
      <span class="spacer"></span>
      <Button variant="ghost" onclick={() => (editing = null)}>Cancel</Button>
      <Button variant="primary" onclick={save} disabled={saving || !name.trim() || !loopId || !wsPath}>
        {editing === "new" ? "Create agent" : "Save"}
      </Button>
    </div>
  </div>

  {#if live}
    <AgentAccess agent={live} {gotoTools} />
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
            {(a.tools ?? []).length ? `${(a.tools ?? []).length} tool${(a.tools ?? []).length === 1 ? "" : "s"}` : "No tools"}
            · {a.network ? "internet allowed" : "offline"}
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
</style>
