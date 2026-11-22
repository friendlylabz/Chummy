<script lang="ts">
  import { tick } from "svelte";
  import { commands, type Agent } from "$lib/ipc";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Button from "$lib/components/Button.svelte";

  let { agent }: { agent: Agent } = $props();

  let scroller: HTMLDivElement;
  const run = $derived(runs.forAgent(agent.id));

  // Keep the newest message in view.
  $effect(() => {
    void chat.messages.length;
    void run?.streaming;
    void run?.replies.length;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" }));
  });
</script>

<div class="scroller" bind:this={scroller}>
  <div class="thread">
    {#if !chat.messages.length && !run}
      <div class="hello">
        <div class="avatar big"><Icon name="bot" size={22} /></div>
        <h2>Chat with {agent.name}</h2>
        <p>Messages run through its loop, inside its own workspace folder.</p>
      </div>
    {/if}

    {#each chat.messages as m (m.id)}
      {#if m.role === "user"}
        <div class="msg user"><div class="bubble" data-selectable>{m.text}</div></div>
      {:else if m.role === "assistant"}
        <div class="msg assistant">
          <div class="avatar"><Icon name="bot" size={15} /></div>
          <div class="body">
            <div class="text" data-selectable>{m.text}</div>
            {#if m.run}<div class="meta">{m.run.llm_calls} model call{m.run.llm_calls === 1 ? "" : "s"} · {m.run.steps} steps</div>{/if}
          </div>
        </div>
      {:else}
        <div class="msg notice">
          <Icon name={m.run?.status === "stopped" ? "stop" : "alert"} size={14} />
          <span data-selectable>{m.text}</span>
        </div>
      {/if}
    {/each}

    {#if run}
      <div class="msg assistant live">
        <div class="avatar"><Icon name="bot" size={15} /></div>
        <div class="body">
          {#if run.replies.length}<div class="text" data-selectable>{run.replies.join("\n\n")}</div>{/if}
          <div class="activity">
            <span class="pulse"></span>
            <span class="step">{run.currentLabel ? `${run.currentLabel}…` : "Working…"}</span>
            <Button variant="ghost" icon="stop" onclick={() => commands.stopRun(run.info.run_id)}>Stop</Button>
          </div>
          {#if run.streaming}<div class="stream">{run.streaming.slice(-600)}</div>{/if}
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .thread {
    width: min(760px, 100%);
    margin: 0 auto;
    padding: var(--space-5) var(--space-5) var(--space-3);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .hello {
    margin: 12vh auto 0;
    text-align: center;
    display: grid;
    justify-items: center;
    gap: var(--space-2);
  }
  .hello h2 {
    font-size: var(--text-lg);
    font-weight: 650;
  }
  .hello p {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .msg {
    display: flex;
    gap: var(--space-3);
  }
  .user {
    justify-content: flex-end;
  }
  .bubble {
    max-width: 80%;
    padding: var(--space-2) var(--space-4);
    border-radius: var(--radius-xl) var(--radius-xl) var(--radius-sm) var(--radius-xl);
    background: var(--accent-soft);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .avatar {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 9px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-muted);
  }
  .avatar.big {
    width: 44px;
    height: 44px;
    border-radius: var(--radius-lg);
    background: var(--accent-soft);
    color: var(--accent);
    border: none;
  }
  .body {
    flex: 1;
    min-width: 0;
    padding-top: 3px;
  }
  .text {
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.6;
  }
  .meta {
    margin-top: var(--space-1);
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .notice {
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .notice :global(svg) {
    flex: none;
    color: var(--warn);
  }
  .activity {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .activity .step {
    flex: 1;
  }
  .pulse {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .stream {
    margin-top: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-left: 2px solid var(--border-strong);
    color: var(--text-faint);
    font-size: var(--text-sm);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 160px;
    overflow: hidden;
    mask-image: linear-gradient(to bottom, transparent, black 40px);
  }
</style>
