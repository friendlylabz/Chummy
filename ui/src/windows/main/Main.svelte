<script lang="ts">
  import { onMount } from "svelte";
  import { setup } from "$lib/stores/setup.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import MainMenu from "./MainMenu.svelte";
  import Onboarding from "./Onboarding.svelte";
  import Composer from "./Composer.svelte";
  import SessionsPanel from "./SessionsPanel.svelte";

  let menuOpen = $state(false);
  let sessionsOpen = $state(true);

  onMount(() => {
    setup.init();
    // Narrow windows start with the sessions panel tucked away.
    if (window.innerWidth < 900) sessionsOpen = false;
  });
</script>

<div class="shell">
  <div class="center">
    <header class="topbar">
      <Button variant="ghost" icon="menu" iconOnly aria-label="Open menu" onclick={() => (menuOpen = true)} />
      <div class="agent" title="Active agent">
        <Icon name="bot" size={15} />
        <span>{setup.ready ? "Agent" : "No agent"}</span>
      </div>
      <div class="spacer"></div>
      {#if !sessionsOpen}
        <Button
          variant="ghost"
          icon="panel-right"
          iconOnly
          aria-label="Show sessions"
          onclick={() => (sessionsOpen = true)}
        />
      {/if}
    </header>

    <main class="chat">
      <div class="transcript">
        {#if setup.loaded && !setup.ready}
          <Onboarding />
        {/if}
      </div>
      <div class="composer-wrap">
        <Composer />
      </div>
    </main>
  </div>

  <SessionsPanel bind:open={sessionsOpen} />
  <MainMenu bind:open={menuOpen} />
</div>

<style>
  .shell {
    display: flex;
    height: 100%;
  }
  .center {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: var(--topbar-h);
    padding: 0 var(--space-2);
  }
  .agent {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-full);
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .spacer {
    flex: 1;
  }

  .chat {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .transcript {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    padding: var(--space-5);
  }
  .composer-wrap {
    flex: none;
    padding: 0 var(--space-5) var(--space-5);
  }
</style>
