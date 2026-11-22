<script lang="ts">
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let { open = $bindable(true) }: { open: boolean } = $props();
</script>

<aside class="panel" class:open aria-label="Sessions" aria-hidden={!open} inert={!open}>
  <div class="inner">
    <div class="head">
      <span class="title">Sessions</span>
      <button class="collapse" type="button" aria-label="Hide sessions" onclick={() => (open = false)}>
        <Icon name="panel-right" size={16} />
      </button>
    </div>

    <div class="group">
      <div class="group-title">Running</div>
      <EmptyState icon="play" title="No agents running" compact>
        Chats and scheduled jobs show up here while they work.
      </EmptyState>
    </div>

    <div class="group">
      <div class="group-title">Recent</div>
      <EmptyState icon="history" title="Nothing yet" compact />
    </div>
  </div>
</aside>

<style>
  .panel {
    flex: none;
    width: 0;
    overflow: hidden;
    border-left: 1px solid transparent;
    background: var(--surface);
    transition:
      width var(--dur-med) var(--ease),
      border-color var(--dur-med) var(--ease);
  }
  .panel.open {
    width: var(--sessions-w);
    border-left-color: var(--border);
  }
  .inner {
    width: var(--sessions-w);
    height: 100%;
    overflow-y: auto;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: var(--topbar-h);
    padding: 0 var(--space-2) 0 var(--space-4);
  }
  .title {
    font-weight: 600;
    font-size: var(--text-sm);
  }
  .collapse {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .collapse:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .group {
    padding: var(--space-2) var(--space-2) 0;
  }
  .group-title {
    padding: var(--space-2) var(--space-2) 0;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
</style>
