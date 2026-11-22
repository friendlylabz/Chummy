<script lang="ts">
  import { onMount } from "svelte";
  import { setup } from "$lib/stores/setup.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import Button from "$lib/components/Button.svelte";
  import MainMenu from "./MainMenu.svelte";
  import Onboarding from "./Onboarding.svelte";
  import Composer from "./Composer.svelte";
  import ChatView from "./ChatView.svelte";
  import AgentPicker from "./AgentPicker.svelte";
  import SessionsPanel from "./SessionsPanel.svelte";

  let menuOpen = $state(false);
  let sessionsOpen = $state(true);

  const agent = $derived(settings.current.agents.find((a) => a.id === settings.current.active_agent) ?? null);

  onMount(() => {
    setup.init();
    chat.init();
    runs.init();
    // Narrow windows start with the sessions panel tucked away.
    if (window.innerWidth < 900) sessionsOpen = false;
  });

  // Load the conversation whenever the active agent changes.
  $effect(() => {
    chat.show(agent?.id ?? null);
  });
</script>

<div class="shell">
  <div class="center">
    <header class="topbar">
      <Button variant="ghost" icon="menu" iconOnly aria-label="Open menu" onclick={() => (menuOpen = true)} />
      <AgentPicker />
      <div class="spacer"></div>
      {#if !sessionsOpen}
        <Button variant="ghost" icon="panel-right" iconOnly aria-label="Show sessions" onclick={() => (sessionsOpen = true)} />
      {/if}
    </header>

    <main class="chat">
      {#if setup.loaded && setup.ready && agent}
        <ChatView {agent} />
      {:else}
        <div class="transcript">
          {#if setup.loaded}<Onboarding />{/if}
        </div>
      {/if}
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
    padding: var(--space-2) var(--space-5) var(--space-5);
  }
</style>
