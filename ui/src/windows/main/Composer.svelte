<script lang="ts">
  import { setup } from "$lib/stores/setup.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let { onsend }: { onsend?: (text: string) => void } = $props();

  let text = $state("");
  let input: HTMLTextAreaElement;

  const disabled = $derived(!setup.ready);
  const placeholder = $derived.by(() => {
    if (!setup.status.model_configured && !setup.status.loop_configured)
      return "Connect a model and add an agent loop to start chatting";
    if (!setup.status.model_configured) return "Connect a model to start chatting";
    if (!setup.status.loop_configured) return "Add an agent loop to start chatting";
    return "Message your agent…";
  });

  function send() {
    const msg = text.trim();
    if (disabled || !msg) return;
    onsend?.(msg);
    text = "";
    resize();
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
  <button type="submit" class="send" disabled={disabled || !text.trim()} aria-label="Send">
    <Icon name="send" size={17} />
  </button>
</form>

<style>
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
    transition: opacity var(--dur-fast) var(--ease);
  }
  .send:disabled {
    background: var(--surface-hover);
    color: var(--text-faint);
    cursor: default;
  }
</style>
