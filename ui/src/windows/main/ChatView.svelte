<script lang="ts">
  import Formatted from "$lib/components/Formatted.svelte";
  import { tick } from "svelte";
  import { stopwatch } from "$lib/format";
  import { commands, type Agent } from "$lib/ipc";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs, type Part } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Button from "$lib/components/Button.svelte";
  import AskCard from "$lib/components/AskCard.svelte";
  import MessageSteps from "./MessageSteps.svelte";
  import type { ChatMessage } from "$lib/ipc";
  import type { Snippet } from "svelte";

  let { agent, footer }: { agent: Agent; footer?: Snippet } = $props();

  /** The message (or response) whose trash can was clicked: it asks "Delete?" first. */
  let confirming = $state<string | null>(null);

  function confirmDelete(ids: string[]) {
    confirming = null;
    chat.deleteMessages(ids);
  }

  /**
   * What the chat shows: your messages one by one, and each run as ONE
   * response (its replies, questions, your answers and any notice), with
   * one set of figures and one delete. Older messages with no run stand alone.
   */
  type Item =
    | { kind: "single"; key: string; m: ChatMessage }
    | { kind: "run"; key: string; runId: string; messages: ChatMessage[] };
  const items = $derived.by(() => {
    const out: Item[] = [];
    for (const m of chat.messages) {
      const runId = m.role !== "user" ? m.run?.run_id : undefined;
      const last = out.at(-1);
      if (runId && last?.kind === "run" && last.runId === runId) last.messages.push(m);
      else if (runId) out.push({ kind: "run", key: m.id, runId, messages: [m] });
      else out.push({ kind: "single", key: m.id, m });
    }
    return out;
  });
  const partsOf = (ms: ChatMessage[]): Part[] =>
    ms.map((m) => ({ role: m.role === "user" ? "answer" : m.role, text: m.text }));

  let scroller: HTMLDivElement;
  const run = $derived(runs.forChat(chat.chatId));
  const jobRun = $derived(runs.jobForAgent(agent.id));
  const waiting = $derived(asks.forChat(agent.id, chat.chatId));
  /** The chat's own question, answered from the message box. */
  const composerAsk = $derived(
    run ? waiting.find((p) => p.origin.run_id === run.info.run_id && p.ask.kind === "question") : undefined,
  );

  function counts(r: NonNullable<ChatMessage["run"]>): string {
    const n = (x: number, one: string) => `${x.toLocaleString("en-US")} ${one}${x === 1 ? "" : "s"}`;
    const pct = (a: number, b: number) => {
      const v = (a / b) * 100;
      return `${v > 0 && v < 1 ? "<1" : Math.round(v)}%`;
    };
    // How long it took comes first (replies saved before it was recorded have none).
    const parts = [r.ms ? stopwatch(r.ms) : "", n(r.llm_calls, "model call"), r.tool_calls ? n(r.tool_calls, "tool call") : "", n(r.steps, "step")];
    // Token figures (estimates), for replies recorded since they were added.
    if (r.tokens) parts.push(`${n(r.tokens, "token")} used`);
    if (r.context_window) parts.push(`context ${pct(r.context_tokens ?? 0, r.context_window)}`);
    // Only for models with a daily limit; unlimited ones show nothing.
    if (r.daily_allowed) parts.push(`daily usage ${pct(r.daily_used ?? 0, r.daily_allowed)}`);
    return parts.filter(Boolean).join(" · ");
  }

  // The live run's timer, to a tenth of a second.
  let nowMs = $state(Date.now());
  $effect(() => {
    if (!run) return;
    nowMs = Date.now();
    const t = setInterval(() => (nowMs = Date.now()), 100);
    return () => clearInterval(t);
  });
  const elapsed = $derived(run ? stopwatch(nowMs - run.startedMs) : "");

  // Keep the newest message in view, but only while you're at the bottom:
  // scrolled up (say, reading What happened), the chat stays put.
  // Only scrolling up leaves the bottom, so a smooth scroll on its way
  // down (or text arriving mid-way) doesn't count as you scrolling away.
  let atBottom = true;
  let lastTop = 0;
  function onScroll() {
    if (!scroller) return;
    const top = scroller.scrollTop;
    if (scroller.scrollHeight - top - scroller.clientHeight < 80) atBottom = true;
    else if (top < lastTop) atBottom = false;
    lastTop = top;
  }
  $effect(() => {
    void chat.chatId;
    atBottom = true;
  });
  // While a What happened is open, new events don't move the chat (its
  // log is newest first, at the top); only a message you send does.
  let seen = 0;
  $effect(() => {
    const n = chat.messages.length;
    void run?.streaming;
    void run?.replies.length;
    void waiting.length;
    const sent = n > seen && chat.messages[n - 1]?.role === "user";
    seen = n;
    if (sent) atBottom = true;
    if (!atBottom) return;
    if (!sent && scroller?.querySelector('.steps .toggle[aria-expanded="true"]')) return;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" }));
  });
</script>

<div class="scroller" bind:this={scroller} onscroll={onScroll}>
  <div class="thread">
    {#if !chat.messages.length && !run}
      <div class="hello">
        <div class="avatar big"><Icon name="bot" size={22} /></div>
        <h2>Chat with {agent.name}</h2>
        <p>Messages run through its loop, inside its own workspace folder.</p>
      </div>
    {/if}

    {#snippet del(key: string, ids: string[], what: string)}
      {#if confirming === key && !run}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions (Escape cancels) -->
        <span
          class="confirm"
          role="group"
          aria-label={`Delete this ${what}?`}
          onkeydown={(e) => {
            if (e.key === "Escape") confirming = null;
          }}
        >
          Delete this {what}?
          <!-- svelte-ignore a11y_autofocus -->
          <button type="button" class="danger" autofocus onclick={() => confirmDelete(ids)}>Delete</button>
          <button type="button" onclick={() => (confirming = null)}>Cancel</button>
        </span>
      {:else}
        <button
          type="button"
          class="trash"
          disabled={!!run}
          title={run ? "Wait until this chat is done to delete messages" : `Delete this ${what} (the agent won't see it either)`}
          aria-label={`Delete ${what}`}
          onclick={() => (confirming = key)}
        >
          <Icon name="trash" size={13} />
        </button>
      {/if}
    {/snippet}

    <!-- One run's response, as it builds and once saved: replies, questions
         asked along the way with your answers, and how it ended. -->
    {#snippet response(parts: Part[], status?: string)}
      {#each parts as p, i (i)}
        {#if p.role === "assistant"}
          <Formatted text={p.text} />
        {:else if p.role === "question"}
          <div class="part asked-part"><Icon name="ask" size={14} /><div><Formatted text={p.text} /></div></div>
        {:else if p.role === "answer"}
          <div class="part answer-part"><span class="who">You</span><span data-selectable>{p.text}</span></div>
        {:else}
          <div class="msg notice in-response">
            <Icon name={status === "stopped" ? "stop" : "alert"} size={14} />
            <span data-selectable>{p.text}</span>
          </div>
        {/if}
      {/each}
    {/snippet}

    {#each items as item (item.key)}
      <!-- svelte-ignore a11y_no_static_element_interactions (leaving cancels "Delete?") -->
      <div
        class="item"
        onmouseleave={() => {
          if (confirming === item.key) confirming = null;
        }}
      >
        {#if item.kind === "run"}
          {@const summary = item.messages.at(-1)!.run!}
          {@const ids = item.messages.map((m) => m.id)}
          <div class="msg assistant">
            <div class="avatar"><Icon name="bot" size={15} /></div>
            <div class="body">
              {@render response(partsOf(item.messages), summary.status)}
              <MessageSteps runId={item.runId} summary={counts(summary)}>
                {#snippet end()}{@render del(item.key, ids, "response")}{/snippet}
              </MessageSteps>
            </div>
          </div>
        {:else if item.m.role === "user" || item.m.role === "answer"}
          <div class="msg user">
            <div class="bubble" class:answer={item.m.role === "answer"} data-selectable>{item.m.text}</div>
          </div>
          <div class="meta-row mine">{@render del(item.key, [item.m.id], "message")}</div>
        {:else if item.m.role === "assistant" || item.m.role === "question"}
          <div class="msg assistant" class:asked={item.m.role === "question"}>
            <div class="avatar"><Icon name={item.m.role === "question" ? "ask" : "bot"} size={15} /></div>
            <div class="body">
              {#if item.m.from}<div class="from">{item.m.from} asked</div>{/if}
              <Formatted text={item.m.text} />
              <div class="meta-row">{@render del(item.key, [item.m.id], "message")}</div>
            </div>
          </div>
        {:else}
          <div class="notice-wrap">
            <div class="msg notice">
              <Icon name="alert" size={14} />
              <span data-selectable>{item.m.text}</span>
            </div>
            <div class="meta-row">{@render del(item.key, [item.m.id], "message")}</div>
          </div>
        {/if}
      </div>
    {/each}

    {#if run}
      <div class="msg assistant live">
        <div class="avatar"><Icon name="bot" size={15} /></div>
        <div class="body">
          {@render response(run.parts)}
          <div class="activity">
            <span class="pulse"></span>
            <span class="step">{run.currentLabel ? `${run.currentLabel}…` : "Working…"}</span>
            <span class="timer" title="Time since this run started">{elapsed}</span>
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
            {chat.undoable.length > 1
              ? `${chat.undoable.length} deleted.`
              : chat.undoable[0].length > 1
                ? "Response deleted."
                : "Message deleted."}
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
  .meta-row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-top: var(--space-1);
  }
  .meta-row.mine {
    justify-content: flex-end;
    margin-top: 2px;
  }
  .meta-row > :global(:last-child) {
    margin-left: auto;
  }
  .trash {
    display: inline-grid;
    place-items: center;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-faint);
    cursor: pointer;
  }
  .trash:hover:not(:disabled) {
    color: var(--accent);
  }
  .trash:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .confirm {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    color: var(--text-muted);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .confirm button,
  .undo button {
    padding: 0 var(--space-2);
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
  .part {
    display: flex;
    gap: var(--space-2);
    margin: var(--space-2) 0;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
  }
  .asked-part {
    background: var(--surface-2);
  }
  .asked-part :global(svg) {
    flex: none;
    margin-top: 3px;
    color: var(--accent);
  }
  .answer-part {
    border: 1px dashed color-mix(in srgb, var(--accent) 50%, transparent);
  }
  .answer-part .who {
    flex: none;
    color: var(--text-faint);
    font-weight: 600;
  }
  .notice.in-response {
    display: flex;
    margin: var(--space-2) 0;
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
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .activity .timer {
    flex: 1;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
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
