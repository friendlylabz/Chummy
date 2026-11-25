<script lang="ts">
  import { commands } from "$lib/ipc";
  import { setup } from "$lib/stores/setup.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import { paused } from "$lib/stores/paused.svelte";
  import { errorText } from "$lib/ipc";
  import Icon from "$lib/components/Icon.svelte";
  import { tick } from "svelte";
  import { TextUndo, undoKey } from "$lib/undo";

  let text = $state("");
  /** Ctrl+Z / Cmd+Z and redo, kept by the app (see $lib/undo). */
  const history = new TextUndo();
  let input: HTMLTextAreaElement;

  const agent = $derived(settings.current.agents.find((a) => a.id === settings.current.active_agent) ?? null);
  const run = $derived(agent ? runs.forChat(chat.chatId) : undefined);
  const jobRun = $derived(agent ? runs.jobForAgent(agent.id) : undefined);
  /** The chat run is waiting for an answer, which this box sends. */
  const question = $derived(
    run ? asks.forRun(run.info.run_id).find((p) => p.ask.kind === "question") : undefined,
  );
  /** This chat's paused run, if it has one. */
  const pausedHere = $derived(agent && !run ? paused.forChat(agent.id, chat.chatId) : undefined);
  const disabled = $derived(!setup.ready || !agent || (!!jobRun && !question) || !!pausedHere);
  let pauseError = $state("");
  async function pause() {
    if (!run) return;
    pauseError = "";
    try {
      await runs.pause(run.info.run_id);
    } catch (e) {
      pauseError = errorText(e);
    }
  }
  const placeholder = $derived.by(() => {
    const s = setup.status;
    const missing = [
      !s.model_configured && "connect a model",
      !s.loop_configured && "add a loop",
      !s.agent_configured && "set up an agent",
    ].filter(Boolean) as string[];
    if (missing.length) {
      const list = missing.length > 1 ? `${missing.slice(0, -1).join(", ")} and ${missing.at(-1)}` : missing[0];
      return `To start chatting, ${list}`;
    }
    if (question) return `Answer ${agent?.name ?? "the agent"}…`;
    if (pausedHere)
      return pausedHere.reason
        ? "Paused: the model couldn't be reached. Resume when it's back, or discard."
        : "Paused. Resume to carry on where it stopped, or discard.";
    if (jobRun && agent) return `${agent.name} is busy with the job “${jobRun.info.title}”…`;
    return agent ? `Message ${agent.name}…` : "Choose an agent to chat with";
  });

  async function send() {
    const msg = text.trim();
    if (question && msg) {
      if (await asks.answer(question.id, { kind: "text", text: msg })) sent();
      return;
    }
    if (disabled || !msg || run) return;
    if (await chat.send(msg)) sent();
  }

  function sent() {
    text = "";
    history.clear();
  }

  function current() {
    return { value: input.value, start: input.selectionStart ?? 0, end: input.selectionEnd ?? 0 };
  }

  async function step(which: "undo" | "redo") {
    const s = which === "undo" ? history.undo(current()) : history.redo(current());
    if (!s) return;
    text = s.value;
    await tick();
    input.setSelectionRange(s.start, s.end);
    resize();
  }

  function onBeforeInput(e: InputEvent) {
    // The webview's own Undo (e.g. from a menu) goes through ours instead.
    if (e.inputType === "historyUndo" || e.inputType === "historyRedo") {
      e.preventDefault();
      step(e.inputType === "historyUndo" ? "undo" : "redo");
      return;
    }
    history.before(current(), e.inputType, e.data);
  }

  function onKeydown(e: KeyboardEvent) {
    const k = undoKey(e);
    if (k) {
      e.preventDefault();
      step(k);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  // Fit the box to its text, also after it's cleared on send (the DOM value
  // only changes after this runs, so measuring right away kept it tall).
  $effect(() => {
    void text;
    resize();
  });

  function resize() {
    if (!input) return;
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 200) + "px";
  }
</script>

{#if chat.error}<div class="error" role="alert">{chat.error}</div>{/if}
{#if question && asks.errors[question.id]}<div class="error" role="alert">{asks.errors[question.id]}</div>{/if}
{#if pauseError}<div class="error" role="alert">{pauseError}</div>{/if}
{#if pausedHere && paused.errors[pausedHere.chat_id]}<div class="error" role="alert">{paused.errors[pausedHere.chat_id]}</div>{/if}
<form
  class="composer"
  class:disabled
  onsubmit={(e) => {
    e.preventDefault();
    send();
  }}
>
  <textarea
    bind:this={input}
    bind:value={text}
    oninput={resize}
    onbeforeinput={onBeforeInput}
    onkeydown={onKeydown}
    {placeholder}
    {disabled}
    rows="1"
    aria-label="Message"
  ></textarea>
  {#if run && !question}
    {#if run.info.pausable}
      <button
        type="button"
        class="send pause"
        aria-label={run.pausing ? "Pausing after this step" : "Pause"}
        title={run.pausing ? "Pausing after this step…" : "Pause (carry on later, even after quitting)"}
        disabled={run.pausing}
        onclick={pause}
      >
        <Icon name="pause" size={15} />
      </button>
    {/if}
    <button type="button" class="send stop" aria-label="Stop" title="Stop" onclick={() => commands.stopRun(run.info.run_id)}>
      <Icon name="stop" size={15} />
    </button>
  {:else if pausedHere}
    <button
      type="button"
      class="send discard"
      aria-label="Discard the paused run"
      title="Discard the paused run"
      disabled={paused.busy[pausedHere.chat_id]}
      onclick={() => paused.discard(pausedHere)}
    >
      <Icon name="trash" size={15} />
    </button>
    <button
      type="button"
      class="send"
      aria-label="Resume"
      title="Resume where it stopped"
      disabled={paused.busy[pausedHere.chat_id]}
      onclick={() => paused.resume(pausedHere)}
    >
      <Icon name="play" size={16} />
    </button>
  {:else}
    <button type="submit" class="send" disabled={disabled || !text.trim()} aria-label="Send">
      <Icon name="send" size={17} />
    </button>
  {/if}
</form>

<style>
  .error {
    width: var(--chat-w);
    margin: 0 auto var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    font-size: var(--text-sm);
  }
  .composer {
    display: flex;
    align-items: flex-end;
    gap: var(--space-2);
    width: var(--chat-w);
    margin: 0 auto;
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
    border-radius: var(--radius-xl);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-sm);
    transition:
      border-color var(--dur-fast) var(--ease),
      opacity var(--dur-fast) var(--ease);
  }
  .composer:focus-within {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border-strong));
  }
  .composer.disabled {
    background: var(--surface-2);
    border-color: var(--border);
    box-shadow: none;
  }
  textarea {
    flex: 1;
    min-height: 36px;
    max-height: 200px;
    padding: 7px 0;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    line-height: 1.5;
  }
  textarea::placeholder {
    color: var(--text-faint);
  }
  textarea:disabled {
    cursor: not-allowed;
  }
  .send {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border: none;
    border-radius: var(--radius-lg);
    background: var(--accent);
    color: var(--on-accent);
    cursor: pointer;
  }
  .send:disabled {
    background: var(--surface-hover);
    color: var(--text-faint);
    cursor: default;
  }
  .send.stop {
    background: var(--text);
    color: var(--bg);
  }
  .send.pause,
  .send.discard {
    background: var(--surface-2);
    color: var(--text);
    border: 1px solid var(--border-strong);
  }
  .send.pause:hover:not(:disabled),
  .send.discard:hover:not(:disabled) {
    background: var(--surface-hover);
  }
  .send.pause:disabled {
    background: var(--surface-2);
    color: var(--text-faint);
    cursor: progress;
  }
</style>
