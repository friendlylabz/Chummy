<script lang="ts">
  import { commands } from "$lib/ipc";
  import { setup } from "$lib/stores/setup.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import Icon, { type IconName } from "$lib/components/Icon.svelte";
  import ThemeSwitch from "$lib/components/ThemeSwitch.svelte";

  let { open = $bindable(false) }: { open: boolean } = $props();

  interface Item {
    label: string;
    icon: IconName;
    run?: () => unknown;
    disabled?: boolean;
    hint?: string;
  }

  // Everything the app offers is reachable from here.
  const sections: { title: string; items: Item[] }[] = $derived([
    {
      title: "Chat",
      items: [
        {
          label: "New chat",
          icon: "plus",
          disabled: !setup.ready || !settings.current.active_agent,
          run: () => settings.current.active_agent && commands.clearChat(settings.current.active_agent),
        },
      ],
    },
    {
      title: "Agents",
      items: [
        { label: "Agents", icon: "bot", run: () => commands.openWindow("settings", "agents") },
        { label: "Loop editor", icon: "flow", run: () => commands.openWindow("editor") },
        { label: "Import loop…", icon: "import", run: () => commands.openWindow("editor", "import") },
      ],
    },
    {
      title: "Automation",
      items: [
        { label: "Scheduler", icon: "clock", run: () => commands.openWindow("scheduler") },
      ],
    },
  ]);

  function choose(item: Item) {
    if (item.disabled || !item.run) return;
    open = false;
    item.run();
  }

  function onKeydown(e: KeyboardEvent) {
    if (open && e.key === "Escape") open = false;
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="scrim" class:open onclick={() => (open = false)} aria-hidden="true"></div>

<nav class="drawer" class:open aria-label="Main menu" aria-hidden={!open} inert={!open}>
  <div class="head">
    <span class="brand"><span class="dot"></span>Chummy</span>
    <button class="close" type="button" aria-label="Close menu" onclick={() => (open = false)}>
      <Icon name="close" size={16} />
    </button>
  </div>

  <div class="sections">
    {#each sections as section (section.title)}
      <div class="section">
        <div class="section-title">{section.title}</div>
        {#each section.items as item (item.label)}
          <button
            class="item"
            type="button"
            disabled={item.disabled}
            onclick={() => choose(item)}
          >
            <Icon name={item.icon} size={17} />
            <span class="label">{item.label}</span>
            {#if item.hint}<span class="hint">{item.hint}</span>{/if}
          </button>
        {/each}
      </div>
    {/each}
  </div>

  <div class="foot">
    <button
      class="item"
      type="button"
      onclick={() => choose({ label: "Settings", icon: "sliders", run: () => commands.openWindow("settings") })}
    >
      <Icon name="sliders" size={17} />
      <span class="label">Settings</span>
    </button>
    <div class="theme"><ThemeSwitch /></div>
  </div>
</nav>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--dur-med) var(--ease);
    z-index: 20;
  }
  .scrim.open {
    opacity: 1;
    pointer-events: auto;
  }

  .drawer {
    position: fixed;
    top: 0;
    bottom: 0;
    left: 0;
    width: var(--drawer-w);
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border-right: 1px solid var(--border);
    transform: translateX(-102%);
    visibility: hidden;
    transition:
      transform var(--dur-med) var(--ease),
      visibility 0s linear var(--dur-med);
    z-index: 21;
  }
  .drawer.open {
    transform: none;
    visibility: visible;
    box-shadow: var(--shadow-lg);
    transition: transform var(--dur-med) var(--ease);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: var(--topbar-h);
    padding: 0 var(--space-3) 0 var(--space-4);
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--accent);
  }
  .close {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .close:hover {
    background: var(--surface-hover);
    color: var(--text);
  }

  .sections {
    flex: 1;
    overflow-y: auto;
    padding: var(--space-2) var(--space-2);
  }
  .section + .section {
    margin-top: var(--space-4);
  }
  .section-title {
    padding: 0 var(--space-3) var(--space-1);
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    height: 36px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text);
    font-size: var(--text-md);
    text-align: left;
    cursor: pointer;
  }
  .item :global(svg) {
    color: var(--text-muted);
  }
  .item:not(:disabled):hover {
    background: var(--surface-hover);
  }
  .item:disabled {
    color: var(--text-faint);
    cursor: default;
  }
  .item:disabled :global(svg) {
    color: var(--text-faint);
  }
  .label {
    flex: 1;
  }
  .hint {
    font-size: var(--text-xs);
    color: var(--text-faint);
  }

  .foot {
    padding: var(--space-2);
    border-top: 1px solid var(--border);
  }
  .theme {
    padding: var(--space-2) var(--space-3) var(--space-1);
  }
</style>
