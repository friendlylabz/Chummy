<script lang="ts" generics="T extends string">
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    items,
    active = $bindable(),
    label,
    onselect,
  }: {
    items: { id: T; label: string; icon: IconName }[];
    active: T;
    label: string;
    /** If given, called instead of changing `active` directly. */
    onselect?: (id: T) => void;
  } = $props();
</script>

<nav class="nav" aria-label={label}>
  {#each items as item (item.id)}
    <button
      type="button"
      class:active={active === item.id}
      aria-current={active === item.id ? "page" : undefined}
      onclick={() => (onselect ? onselect(item.id) : (active = item.id))}
    >
      <Icon name={item.icon} size={16} />
      <span>{item.label}</span>
    </button>
  {/each}
</nav>

<style>
  .nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-3) var(--space-2);
  }
  button {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    height: 34px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-md);
    text-align: left;
    cursor: pointer;
  }
  button:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  button.active {
    background: var(--surface-hover);
    color: var(--text);
    font-weight: 550;
  }
</style>
