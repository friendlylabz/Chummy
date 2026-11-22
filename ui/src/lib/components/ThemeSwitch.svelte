<script lang="ts">
  import type { Theme } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import Icon, { type IconName } from "./Icon.svelte";

  const options: { value: Theme; label: string; icon: IconName }[] = [
    { value: "system", label: "System", icon: "monitor" },
    { value: "light", label: "Light", icon: "sun" },
    { value: "dark", label: "Dark", icon: "moon" },
  ];
</script>

<div class="switch" role="radiogroup" aria-label="Theme">
  {#each options as opt (opt.value)}
    <button
      type="button"
      role="radio"
      aria-checked={settings.current.theme === opt.value}
      onclick={() => settings.setTheme(opt.value)}
    >
      <Icon name={opt.icon} size={15} />
      <span>{opt.label}</span>
    </button>
  {/each}
</div>

<style>
  .switch {
    display: inline-flex;
    padding: 3px;
    gap: 2px;
    border-radius: var(--radius-md);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 550;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  button:hover {
    color: var(--text);
  }
  button[aria-checked="true"] {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
</style>
