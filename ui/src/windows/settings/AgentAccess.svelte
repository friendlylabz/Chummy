<script lang="ts">
  /**
   * An agent's own tool and internet switches. A tool runs only when it is
   * on app-wide; then the agent's switch decides, unless a loop block turns
   * the tool on or off for itself.
   */
  import { onMount } from "svelte";
  import { commands, errorText, type Agent } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { tools } from "$lib/stores/tools.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let { agent, gotoTools }: { agent: Agent; gotoTools: () => void } = $props();

  let error = $state("");
  let busy = $state(false);

  onMount(() => tools.init());

  const perms = $derived(settings.current.permissions);
  const mine = $derived(agent.tools ?? []);

  async function apply(next: string[], network: boolean) {
    busy = true;
    error = "";
    try {
      await commands.setAgentAccess(agent.id, next, network);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
  function toggle(name: string, on: boolean) {
    const rest = mine.filter((t) => t !== name);
    apply(on ? [...rest, name] : rest, !!agent.network);
  }
</script>

<div class="card card-pad">
  <div class="section-title"><Icon name="wrench" size={16} /> Tools and internet for {agent.name}</div>
  <p class="explain">
    A tool can run only when it is on <b>app-wide</b> (Settings → Tools and Permissions). Then <b>this agent's</b>
    switch decides. Loop blocks follow the agent by default; a block can turn a tool on or off just for itself, and
    the run notes it when a block turns on a tool that is off here. Going online also needs Internet access in
    Permissions and the tool's own internet switch, which nothing can override.
  </p>

  {#if !perms.tools}
    <div class="notice warn"><Icon name="alert" size={16} /><span style="flex: 1">Tools are off for the whole app (Permissions), so none of these run yet.</span></div>
  {/if}

  {#if !tools.installed.length}
    <div class="notice">
      <Icon name="wrench" size={16} />
      <span style="flex: 1">No tools are installed yet.</span>
      <Button onclick={gotoTools}>Set up tools</Button>
    </div>
  {:else}
    <div class="rows">
      {#each tools.installed as t (t.name)}
        {@const on = mine.includes(t.name)}
        <label class="row tool">
          <input type="checkbox" checked={on} disabled={busy} onchange={(e) => toggle(t.name, (e.currentTarget as HTMLInputElement).checked)} />
          <span class="t-main">
            <span class="row-title">{t.title}</span>
            <span class="row-sub">{t.description}</span>
          </span>
          <span class="levels" aria-label="Where it is on">
            <span class="lvl" class:yes={perms.tools && t.enabled} title="App-wide (Settings → Tools and Permissions)">App</span>
            <span class="lvl" class:yes={on} title="This agent">Agent</span>
          </span>
        </label>
      {/each}
    </div>
  {/if}

  <label class="row net">
    <input type="checkbox" checked={!!agent.network} disabled={busy} onchange={(e) => apply(mine, (e.currentTarget as HTMLInputElement).checked)} />
    <span class="t-main">
      <span class="row-title">Tools may use the internet</span>
      <span class="row-sub">
        {perms.network ? "Internet access is on in Permissions." : "Internet access is off in Permissions, so tools stay offline even if this is on."}
        Each tool's own internet switch must be on too. Blocks follow this unless they say otherwise.
      </span>
    </span>
  </label>
  {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
</div>

<style>
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
  .rows {
    margin-bottom: var(--space-3);
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    cursor: pointer;
  }
  .net {
    padding-top: var(--space-3);
    border-top: 1px solid var(--border);
  }
  .t-main {
    flex: 1;
    min-width: 0;
    display: grid;
  }
  .levels {
    display: flex;
    gap: 4px;
  }
  .lvl {
    padding: 1px 7px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
    text-decoration: line-through;
  }
  .lvl.yes {
    background: color-mix(in srgb, var(--ok) 15%, transparent);
    color: var(--ok);
    text-decoration: none;
  }
  .notice {
    margin-bottom: var(--space-3);
  }
</style>
