<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    title,
    children,
    actions,
    onclose,
  }: { title: string; children?: Snippet; actions: Snippet; onclose: () => void } = $props();
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" onclick={onclose} aria-hidden="true"></div>
<div class="modal" role="dialog" aria-modal="true" aria-label={title}>
  <h2>{title}</h2>
  {#if children}<div class="body">{@render children()}</div>{/if}
  <div class="actions">{@render actions()}</div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: var(--scrim);
  }
  .modal {
    position: fixed;
    z-index: 51;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(420px, calc(100vw - 32px));
    padding: var(--space-5);
    border-radius: var(--radius-xl);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
  }
  h2 {
    font-size: var(--text-lg);
    font-weight: 650;
  }
  .body {
    margin-top: var(--space-2);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-5);
  }
</style>
