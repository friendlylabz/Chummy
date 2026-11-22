<script lang="ts">
  import { commands } from "$lib/ipc";
  import { date } from "$lib/format";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";

  const ws = $derived(settings.current.workspace);
</script>

<h1>Workspace</h1>
<p class="pane-sub">The one folder agents may use. They can read and change files inside it and can't see anything outside it.</p>

{#if ws}
  <div class="card card-pad">
    <div class="ws">
      <span class="ws-icon"><Icon name="folder" size={18} /></span>
      <div style="min-width: 0">
        <div class="mono path" data-selectable>{ws.path}</div>
        <div class="faint xs">Confirmed {date(ws.confirmed_at)}</div>
      </div>
    </div>
    <div class="actions">
      <Button icon="folder" onclick={() => commands.chooseWorkspace()}>Change folder…</Button>
      <Button variant="ghost" onclick={() => commands.clearWorkspace()}>Stop using this folder</Button>
    </div>
  </div>
{:else}
  <div class="card empty">
    <EmptyState icon="folder" title="No folder chosen">
      Create a folder just for your agents' work, then choose it here. You'll confirm it before it's used.
      {#snippet actions()}
        <Button variant="primary" icon="folder" onclick={() => commands.chooseWorkspace()}>Choose folder…</Button>
      {/snippet}
    </EmptyState>
  </div>
{/if}

<div class="section-label">Never allowed</div>
<div class="card card-pad">
  <ul class="rules">
    <li><Icon name="shield" size={15} />A whole drive, or a folder that contains one of the places below</li>
    <li><Icon name="shield" size={15} />Your home folder itself (folders inside it are fine)</li>
    <li><Icon name="shield" size={15} />System folders, app data, and hidden settings folders such as .ssh</li>
    <li><Icon name="shield" size={15} />Chummy's own folder, where its settings and models live</li>
  </ul>
  <p class="faint xs" style="margin-top: var(--space-3)">
    These rules can't be turned off. Inside the workspace, links that point outside it are blocked too.
  </p>
</div>

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
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
  }
  .ws-icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 36px;
    height: 36px;
    border-radius: var(--radius-md);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .path {
    word-break: break-all;
  }
  .rules {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-2);
    font-size: var(--text-sm);
  }
  .rules li {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }
  .rules li :global(svg) {
    color: var(--text-faint);
    flex: none;
  }
</style>
