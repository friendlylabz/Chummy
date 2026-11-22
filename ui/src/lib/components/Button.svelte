<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Icon, { type IconName } from "./Icon.svelte";

  type Variant = "primary" | "secondary" | "ghost";

  let {
    variant = "secondary",
    icon,
    iconOnly = false,
    children,
    ...rest
  }: HTMLButtonAttributes & {
    variant?: Variant;
    icon?: IconName;
    /** Square button showing only the icon; pass `aria-label`. */
    iconOnly?: boolean;
    children?: Snippet;
  } = $props();
</script>

<button class="btn {variant}" class:icon-only={iconOnly} type="button" {...rest}>
  {#if icon}<Icon name={icon} size={iconOnly ? 18 : 16} />{/if}
  {#if children}<span>{@render children()}</span>{/if}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    height: 34px;
    padding: 0 var(--space-4);
    border: 1px solid transparent;
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
    font-weight: 550;
    white-space: nowrap;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .primary {
    background: var(--accent);
    color: var(--on-accent);
  }
  .primary:not(:disabled):hover {
    background: var(--accent-strong);
  }

  .secondary {
    background: var(--surface-2);
    border-color: var(--border);
  }
  .secondary:not(:disabled):hover {
    background: var(--surface-hover);
    border-color: var(--border-strong);
  }

  .ghost {
    background: transparent;
    color: var(--text-muted);
  }
  .ghost:not(:disabled):hover {
    background: var(--surface-hover);
    color: var(--text);
  }

  .icon-only {
    width: 34px;
    padding: 0;
  }
</style>
