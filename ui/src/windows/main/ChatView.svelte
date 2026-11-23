<script lang="ts">
  import Formatted from "$lib/components/Formatted.svelte";
  import { tick } from "svelte";
  import { commands, type Agent } from "$lib/ipc";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Button from "$lib/components/Button.svelte";
  import AskCard from "$lib/components/AskCard.svelte";
  import MessageSteps from "./MessageSteps.svelte";
  import type { ChatMessage } from "$lib/ipc";
  import type { Snippet } from "svelte";

  let { agent, footer }: { agent: Agent; footer?: Snippet } = $props();

  /** The message whose trash can was clicked: it asks "Delete?" first. */
  let confirming = $state<string | null>(null);

  function confirmDelete(id: string) {
    confirming = null;
    chat.deleteMessage(id);
  }

  let scroller: HTMLDivElement;
  const run = $derived(runs.forChat(chat.chatId));
  const jobRun = $derived(runs.jobForAgent(agent.id));
  const waiting = $derived(asks.forChat(agent.id, chat.chatId));
  /** The chat's own question, answered from the message box. */
  const composerAsk = $derived(
    run ? waiting.find((p) => p.origin.run_id === run.info.run_id && p.ask.kind === "question") : undefined,
  );

  /** Each run's step view goes under its last message. */
  const lastOfRun = $derived.by(() => {
    const last = new Map<string, string>();
    for (const m of chat.messages) if (m.run?.run_id) last.set(m.run.run_id, m.id);
    return new Set(last.values());
  });
  function counts(r: NonNullable<ChatMessage["run"]>): string {
    const n = (x: number, one: string) => `${x.toLocaleString("en-US")} ${one}${x === 1 ? "" : "s"}`;
    const pct = (a: number, b: number) => {
      const v = (a / b) * 100;
      return `${v > 0 && v < 1 ? "<1" : Math.round(v)}%`;
    };
    const parts = [n(r.llm_calls, "model call"), r.tool_calls ? n(r.tool_calls, "tool call") : "", n(r.steps, "step")];
    // Token figures (estimates), for replies recorded since they were added.
    if (r.tokens) parts.push(`${n(r.tokens, "token")} used`);
    if (r.context_window) parts.push(`context ${pct(r.context_tokens ?? 0, r.context_window)}`);
    // Only for models with a daily limit; unlimited ones show nothing.
    if (r.daily_allowed) parts.push(`daily usage ${pct(r.daily_used ?? 0, r.daily_allowed)}`);
    return parts.filter(Boolean).join(" · ");
  }

  // Keep the newest message in view.
  $effect(() => {
    void chat.messages.length;
    void run?.streaming;
    void run?.replies.length;
    void waiting.length;
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
      <!-- svelte-ignore a11y_no_static_element_interactions (hover only: leaving cancels "Delete?") -->
      <div
        class="item"
        class:mine={m.role === "user" || m.role === "answer"}
        class:plain={m.role === "notice"}
        onmouseleave={() => {
          if (confirming === m.id) confirming = null;
        }}
      >
        {#if m.role === "user"}
          <div class="msg user"><div class="bubble" data-selectable>{m.text}</div></div>
        {:else if m.role === "assistant"}
          <div class="msg assistant">
            <div class="avatar"><Icon name="bot" size={15} /></div>
            <div class="body">
              <Formatted text={m.text} />
              {#if m.run?.run_id && lastOfRun.has(m.id)}
                <MessageSteps runId={m.run.run_id} summary={counts(m.run)} />
              {:else if m.run}<div class="meta">{counts(m.run)}</div>{/if}
            </div>
          </div>
        {:else if m.role === "question"}
          <div class="msg assistant asked">
            <div class="avatar"><Icon name="ask" size={15} /></div>
            <div class="body">
              {#if m.from}<div class="from">{m.from} asked</div>{/if}
              <Formatted text={m.text} />
            </div>
          </div>
        {:else if m.role === "answer"}
          <div class="msg user"><div class="bubble answer" data-selectable>{m.text}</div></div>
        {:else}
          <div class="notice-wrap">
            <div class="msg notice">
              <Icon name={m.run?.status === "stopped" ? "stop" : "alert"} size={14} />
              <span data-selectable>{m.text}</span>
            </div>
            {#if m.run?.run_id && lastOfRun.has(m.id)}
              <MessageSteps runId={m.run.run_id} summary={counts(m.run)} />
            {/if}
          </div>
        {/if}
        {#if !run && confirming === m.id}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions (Escape cancels) -->
          <div
            class="confirm"
            role="group"
            aria-label="Delete this message?"
            onkeydown={(e) => {
              if (e.key === "Escape") confirming = null;
            }}
          >
            <span>Delete this message?</span>
            <!-- svelte-ignore a11y_autofocus -->
            <button type="button" class="danger" autofocus onclick={() => confirmDelete(m.id)}>Delete</button>
            <button type="button" onclick={() => (confirming = null)}>Cancel</button>
          </div>
        {:else if !run}
          <button
            type="button"
            class="trash"
            title="Delete this message (the agent won't see it either)"
            aria-label="Delete message"
            onclick={() => (confirming = m.id)}
          >
            <Icon name="trash" size={13} />
          </button>
        {/if}
      </div>
    {/each}

    {#if run}
      <div class="msg assistant live">
        <div class="avatar"><Icon name="bot" size={15} /></div>
        <div class="body">
          {#if run.replies.length}<Formatted text={run.replies.join("\n\n")} />{/if}
          <div class="activity">
            <span class="pulse"></span>
            <span class="step">{run.currentLabel ? `${run.currentLabel}…` : "Working…"}</span>
            <Button variant="ghost" icon="stop" onclick={() => commands.stopRun(run.info.run_id)}>Stop</Button>
          </div>
          {#if run.streaming && !run.waitingForYou}<div class="stream">{run.streaming.slice(-600)}</div>{/if}
          <MessageSteps runId={run.info.run_id} live={run.entries} />
        </div>
      </div>
    {/if}

    {#if jobRun}
      <div class="msg notice job">
        <Icon name="clock" size={14} />
        <span>The job “{jobRun.info.title}” is running as {agent.name}{jobRun.currentLabel ? ` · ${jobRun.currentLabel}` : ""}</span>
        <Button variant="ghost" icon="stop" onclick={() => commands.stopRun(jobRun.info.run_id)}>Stop</Button>
      </div>
    {/if}

    {#each waiting as p (p.id)}
      <AskCard pending={p} showOrigin={p.origin.kind === "job"} answerHere={p.id !== composerAsk?.id} />
    {/each}
  </div>
  {#if footer}
    <div class="footer">
      {#if chat.undoable.length}
        <div class="undo" role="status">
          <span>
            {chat.undoable.length === 1 ? "Message deleted." : `${chat.undoable.length} messages deleted.`}
          </span>
          <button type="button" class="link" onclick={() => chat.undoDelete()}>
            {chat.undoable.length === 1 ? "Undo" : "Undo last"}
          </button>
          <button type="button" class="close" aria-label="Dismiss" onclick={() => chat.dismissUndo()}>
            <Icon name="close" size={12} />
          </button>
        </div>
      {/if}
      {@render footer()}
    </div>
  {/if}
</div>

<style>
  /* The message box sits inside the scroller (sticky at the bottom), so the
     scroll bar runs the full height and messages scroll under the box. */
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }
  .footer {
    position: sticky;
    bottom: 0;
    margin-top: auto;
    padding: var(--space-4) var(--space-5) var(--space-5);
    background: linear-gradient(to bottom, transparent, var(--bg) var(--space-4));
  }
  .item {
    position: relative;
  }
  .trash {
    position: absolute;
    top: 100%;
    left: 40px;
    display: grid;
    place-items: center;
    width: 22px;
    height: 16px;
    padding: 0;
    border: none;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease);
  }
  .confirm {
    position: absolute;
    top: calc(100% - 2px);
    left: 40px;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 3px 4px 3px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
  }
  .item.mine .confirm {
    left: auto;
    right: 0;
  }
  .item.plain .confirm {
    left: 0;
  }
  .confirm button,
  .undo button {
    padding: 2px var(--space-2);
    border: none;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .confirm button:hover,
  .undo button:hover {
    background: var(--surface-2);
  }
  .confirm button.danger {
    color: var(--danger);
    font-weight: 600;
  }
  .undo {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: fit-content;
    margin: 0 auto var(--space-2);
    padding: 4px 4px 4px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
  .undo button.link {
    color: var(--accent);
    font-weight: 600;
  }
  .undo button.close {
    display: grid;
    place-items: center;
    color: var(--text-faint);
  }
  .item.plain .trash {
    left: 0;
  }
  .item.mine .trash {
    left: auto;
    right: 0;
  }
  .item:hover .trash,
  .trash:focus-visible {
    opacity: 1;
  }
  .trash:hover {
    color: var(--danger);
    background: var(--surface-2);
  }
  .thread {
    flex: 1 0 auto;
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
  .notice.job span {
    flex: 1;
  }
  .notice.job :global(svg) {
    color: var(--accent);
  }
  .asked .avatar {
    color: var(--accent);
  }
  .from {
    color: var(--text-faint);
    font-size: var(--text-xs);
    margin-bottom: 2px;
  }
  .bubble.answer {
    border: 1px dashed color-mix(in srgb, var(--accent) 50%, transparent);
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
