<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    icon,
    title,
    children,
    actions,
    compact = false,
  }: {
    icon: IconName;
    title: string;
    children?: Snippet;
    actions?: Snippet;
    compact?: boolean;
  } = $props();
</script>

<div class="empty" class:compact>
  <div class="glyph"><Icon name={icon} size={compact ? 18 : 22} /></div>
  <h3>{title}</h3>
  {#if children}<p>{@render children()}</p>{/if}
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: var(--space-2);
    max-width: 360px;
    margin: auto;
    padding: var(--space-6) var(--space-4);
  }
  .glyph {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    margin-bottom: var(--space-2);
    border-radius: var(--radius-lg);
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-muted);
  }
  h3 {
    font-size: var(--text-md);
    font-weight: 600;
  }
  p {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }

  .compact {
    padding: var(--space-5) var(--space-3);
  }
  .compact .glyph {
    width: 36px;
    height: 36px;
    border-radius: var(--radius-md);
  }
  .compact h3 {
    font-size: var(--text-sm);
  }
  .compact p {
    font-size: var(--text-xs);
  }
</style>
