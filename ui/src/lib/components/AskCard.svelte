<script lang="ts">
  import type { Pending } from "$lib/ipc";
  import { asks } from "$lib/stores/asks.svelte";
  import Button from "./Button.svelte";
  import Icon from "./Icon.svelte";

  let {
    pending,
    showOrigin = true,
    answerHere = true,
  }: {
    pending: Pending;
    /** Say who's asking (a job, an agent, a test run). */
    showOrigin?: boolean;
    /** Show an answer box. Off when the chat's message box answers instead. */
    answerHere?: boolean;
  } = $props();

  let text = $state("");
  let reason = $state("");
  let busy = $state(false);
  let showAll = $state(false);
  const fullArgs = $derived(
    pending.ask.kind === "tool_approval" ? JSON.stringify(pending.ask.arguments, null, 2) : "",
  );

  const approval = $derived(pending.ask.kind === "tool_approval");
  const tool = $derived(pending.ask.kind === "tool_approval" ? pending.ask.tool : "");
  const budget = $derived(pending.ask.kind === "budget" ? pending.ask : null);
  const fmt = (n: number) => n.toLocaleString();
  const who = $derived(
    pending.origin.kind === "job"
      ? `The job “${pending.origin.title}”`
      : pending.origin.kind === "test"
        ? "The test run"
        : pending.origin.title,
  );
  const error = $derived(asks.errors[pending.id]);

  async function reply(answer: Parameters<typeof asks.answer>[1]) {
    busy = true;
    await asks.answer(pending.id, answer);
    busy = false;
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing && text.trim()) {
      e.preventDefault();
      reply({ kind: "text", text: text.trim() });
    }
  }
</script>

<div
  class="card"
  class:approval={approval || !!budget}
  role="group"
  aria-label={approval ? `Allow ${tool}?` : budget ? "Daily token limit reached" : "Question"}
>
  <div class="head">
    <Icon name={approval ? "shield" : budget ? "gauge" : "ask"} size={15} />
    <span>
      {#if budget}
        {showOrigin ? `${who}: ` : ""}{budget.model} has used today's tokens
      {:else if showOrigin}{who}{approval ? " wants to run a tool" : " asks"}{:else}{approval ? "Allow this tool call?" : "Question"}{/if}
    </span>
  </div>
  <div class="text" data-selectable>{pending.ask.text}</div>

  {#if budget}
    <div class="hint">
      Used {fmt(budget.used)} of {fmt(budget.limit)} tokens today. The allowance refreshes at midnight; you can change
      the limit in Settings → Models.
    </div>
    <div class="actions">
      <Button variant="primary" icon="check" disabled={busy} onclick={() => reply({ kind: "allow" })}
        >Allow {fmt(budget.extra)} more today</Button
      >
      <Button icon="stop" disabled={busy} onclick={() => reply({ kind: "deny", text: "" })}>Stop the run</Button>
    </div>
  {:else if approval}
    <button type="button" class="link" aria-expanded={showAll} onclick={() => (showAll = !showAll)}>
      {showAll ? "Hide" : "Show"} the exact arguments
    </button>
    {#if showAll}<pre class="args" data-selectable>{fullArgs}</pre>{/if}
    <input
      class="reason"
      bind:value={reason}
      placeholder="Why not? (optional; the model is told)"
      aria-label="Reason for not allowing"
      disabled={busy}
    />
    <div class="actions">
      <Button variant="primary" icon="check" disabled={busy} onclick={() => reply({ kind: "allow" })}>Allow</Button>
      <Button icon="close" disabled={busy} onclick={() => reply({ kind: "deny", text: reason.trim() })}>Don't allow</Button>
    </div>
  {:else}
    {#if answerHere}
      <div class="answer">
        <textarea
          bind:value={text}
          onkeydown={onKeydown}
          rows="2"
          placeholder="Your answer…"
          aria-label="Your answer"
          disabled={busy}
        ></textarea>
        <Button
          variant="primary"
          icon="send"
          disabled={busy || !text.trim()}
          onclick={() => reply({ kind: "text", text: text.trim() })}>Answer</Button
        >
      </div>
    {/if}
    <div class="hint">
      {answerHere ? "" : "Answer in the message box below. "}Your answer goes to the agent's loop; never type passwords or
      keys here.
    </div>
  {/if}
  {#if error}<div class="error" role="alert">{error}</div>{/if}
</div>

<style>
  .card {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-4);
    border: 1px solid color-mix(in srgb, var(--accent) 45%, var(--border));
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--accent) 7%, var(--surface));
  }
  .card.approval {
    border-color: color-mix(in srgb, var(--warn) 55%, var(--border));
    background: color-mix(in srgb, var(--warn) 7%, var(--surface));
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .head :global(svg) {
    color: var(--accent);
  }
  .approval .head :global(svg) {
    color: var(--warn);
  }
  .text {
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.55;
  }
  .approval .text {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    max-height: 240px;
    overflow: auto;
  }
  .actions,
  .answer {
    display: flex;
    gap: var(--space-2);
    align-items: flex-end;
  }
  textarea,
  .reason {
    flex: 1;
    min-width: 0;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    color: var(--text);
    font: inherit;
    resize: vertical;
  }
  .reason {
    font-size: var(--text-sm);
  }
  .link {
    justify-self: start;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .args {
    margin: 0;
    max-height: 220px;
    overflow: auto;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--bg);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    word-break: break-all;
  }
  .hint {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .error {
    color: var(--danger);
    font-size: var(--text-sm);
  }
</style>
