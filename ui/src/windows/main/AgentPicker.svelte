<script lang="ts">
  import { commands } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let open = $state(false);
  const agents = $derived(settings.current.agents);
  const active = $derived(agents.find((a) => a.id === settings.current.active_agent) ?? null);

  function choose(id: string) {
    open = false;
    commands.setActiveAgent(id);
  }
</script>

<svelte:window onkeydown={(e) => open && e.key === "Escape" && (open = false)} />

<div class="picker">
  <button class="pill" type="button" aria-haspopup="listbox" aria-expanded={open} disabled={!agents.length} onclick={() => (open = !open)}>
    <Icon name="bot" size={15} />
    <span>{active?.name ?? "No agent"}</span>
    {#if agents.length}<Icon name="chevron-down" size={14} />{/if}
  </button>

  {#if open}
    <div class="scrim" onclick={() => (open = false)} aria-hidden="true"></div>
    <div class="menu" role="listbox" aria-label="Agents">
      {#each agents as a (a.id)}
        <button type="button" role="option" aria-selected={a.id === active?.id} onclick={() => choose(a.id)}>
          <span class="name">{a.name}</span>
          {#if runs.forAgent(a.id)}<span class="dot" title="Working"></span>{/if}
          {#if a.id === active?.id}<Icon name="check" size={15} />{/if}
        </button>
      {/each}
      <div class="sep"></div>
      <button type="button" onclick={() => ((open = false), commands.openWindow("settings", "agents"))}>
        <span class="name muted">Manage agents…</span>
      </button>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-full);
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text);
    font-size: var(--text-sm);
    font-weight: 550;
    cursor: pointer;
  }
  .pill:disabled {
    color: var(--text-muted);
    cursor: default;
  }
  .pill:not(:disabled):hover {
    border-color: var(--border-strong);
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 10;
  }
  .menu {
    position: absolute;
    top: 36px;
    left: 0;
    z-index: 11;
    min-width: 220px;
    padding: var(--space-1);
    border-radius: var(--radius-lg);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    height: 34px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    text-align: left;
    cursor: pointer;
  }
  .menu button:hover {
    background: var(--surface-hover);
  }
  .name {
    flex: 1;
  }
  .muted {
    color: var(--text-muted);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .sep {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--border);
  }
</style>
