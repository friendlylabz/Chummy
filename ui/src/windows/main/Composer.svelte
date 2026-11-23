<script lang="ts">
  import { commands } from "$lib/ipc";
  import { setup } from "$lib/stores/setup.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { asks } from "$lib/stores/asks.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let text = $state("");
  let input: HTMLTextAreaElement;

  const agent = $derived(settings.current.agents.find((a) => a.id === settings.current.active_agent) ?? null);
  const run = $derived(agent ? runs.forChat(chat.chatId) : undefined);
  const jobRun = $derived(agent ? runs.jobForAgent(agent.id) : undefined);
  /** The chat run is waiting for an answer, which this box sends. */
  const question = $derived(
    run ? asks.forRun(run.info.run_id).find((p) => p.ask.kind === "question") : undefined,
  );
  const disabled = $derived(!setup.ready || !agent || (!!jobRun && !question));
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
    if (jobRun && agent) return `${agent.name} is busy with the job “${jobRun.info.title}”…`;
    return agent ? `Message ${agent.name}…` : "Choose an agent to chat with";
  });

  async function send() {
    const msg = text.trim();
    if (question && msg) {
      if (await asks.answer(question.id, { kind: "text", text: msg })) {
        text = "";
        resize();
      }
      return;
    }
    if (disabled || !msg || run) return;
    if (await chat.send(msg)) {
      text = "";
      resize();
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  function resize() {
    if (!input) return;
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 200) + "px";
  }
</script>

{#if chat.error}<div class="error" role="alert">{chat.error}</div>{/if}
{#if question && asks.errors[question.id]}<div class="error" role="alert">{asks.errors[question.id]}</div>{/if}
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
    onkeydown={onKeydown}
    {placeholder}
    {disabled}
    rows="1"
    aria-label="Message"
  ></textarea>
  {#if run && !question}
    <button type="button" class="send stop" aria-label="Stop" onclick={() => commands.stopRun(run.info.run_id)}>
      <Icon name="stop" size={15} />
    </button>
  {:else}
    <button type="submit" class="send" disabled={disabled || !text.trim()} aria-label="Send">
      <Icon name="send" size={17} />
    </button>
  {/if}
</form>

<style>
  .error {
    width: min(760px, 100%);
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
    width: min(760px, 100%);
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
</style>
