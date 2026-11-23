<script lang="ts">
  import { onMount } from "svelte";
  import { setup } from "$lib/stores/setup.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import { commands, events, type Activity, type CloseAction } from "$lib/ipc";
  import Button from "$lib/components/Button.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import MainMenu from "./MainMenu.svelte";
  import Onboarding from "./Onboarding.svelte";
  import Composer from "./Composer.svelte";
  import ChatView from "./ChatView.svelte";
  import AgentPicker from "./AgentPicker.svelte";
  import ChatPicker from "./ChatPicker.svelte";
  import SessionsPanel from "./SessionsPanel.svelte";

  let menuOpen = $state(false);
  let sessionsOpen = $state(true);
  /** Set while asking whether to quit. */
  let closing = $state<Activity | null>(null);
  let remember = $state(false);

  function plural(n: number, one: string, many: string) {
    return `${n} ${n === 1 ? one : many}`;
  }

  async function choose(choice: CloseAction) {
    const r = remember;
    closing = null;
    remember = false;
    await commands.closeChoice(choice, r);
  }

  const agent = $derived(settings.current.agents.find((a) => a.id === settings.current.active_agent) ?? null);

  onMount(() => {
    setup.init();
    chat.init();
    runs.init();
    asks.init();
    events.onCloseRequested((a) => (closing = a));
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
      <ChatPicker />
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

{#if closing}
  <Modal title={closing.reason === "quit" ? "Quit Chummy?" : "Close Chummy?"} onclose={() => (closing = null)}>
    {#if closing.runs || closing.waiting}
      <p>
        {[closing.runs && plural(closing.runs, "run is", "runs are") + " still going", closing.waiting && plural(closing.waiting, "question is", "questions are") + " waiting for you"]
          .filter(Boolean)
          .join(", and ")}. Quitting stops {closing.runs + closing.waiting === 1 ? "it" : "them"}.
      </p>
    {/if}
    {#if closing.scheduled && !closing.paused}
      <p>{plural(closing.scheduled, "scheduled job", "scheduled jobs")} won't run while Chummy is closed.</p>
    {/if}
    {#if closing.tray}
      <p class="muted">Keep Chummy running in the tray instead, and {closing.scheduled ? "jobs carry on" : "find it there"}. The tray icon shows it's running.</p>
    {:else}
      <p class="muted">There's no tray on this desktop, so Chummy can only stay open minimized.</p>
    {/if}
    {#if closing.reason === "close"}
      <label class="remember"><input type="checkbox" bind:checked={remember} /> Remember my choice</label>
    {/if}
    {#snippet actions()}
      <Button onclick={() => (closing = null)}>Cancel</Button>
      <Button onclick={() => choose("tray")}>{closing!.tray ? "Keep running in tray" : "Minimize"}</Button>
      <Button variant="primary" onclick={() => choose("quit")}>Quit</Button>
    {/snippet}
  </Modal>
{/if}

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
  .muted {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .remember {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-3);
    font-size: var(--text-sm);
    color: var(--text-muted);
  }
  .composer-wrap {
    flex: none;
    padding: var(--space-2) var(--space-5) var(--space-5);
  }
</style>
