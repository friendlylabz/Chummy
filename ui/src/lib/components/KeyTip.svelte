<script lang="ts">
  /**
   * A small ⓘ that explains encrypted logs and the log key on hover or
   * focus. Used in onboarding and in Settings → Logs.
   */
  import Icon from "./Icon.svelte";

  let { label = "About encrypted logs" }: { label?: string } = $props();
  let open = $state(false);
  /** Show below the button when there isn't room above. */
  let below = $state(false);
  let btn = $state<HTMLButtonElement>();
  function show() {
    below = (btn?.getBoundingClientRect().top ?? 999) < 360;
    open = true;
  }
</script>

<span class="tip-wrap">
  <button
    type="button"
    class="tip-btn"
    aria-label={label}
    aria-expanded={open}
    bind:this={btn}
    onmouseenter={show}
    onmouseleave={() => (open = false)}
    onfocus={show}
    onblur={() => (open = false)}
    onclick={() => (open ? (open = false) : show())}
  >
    <Icon name="info" size={14} />
  </button>
  {#if open}
    <span class="tip" class:below role="tooltip">
      <strong>Your run logs are encrypted.</strong>
      Chummy logs every step a run takes, so you can see what an agent did. Those logs can contain your messages,
      the agent's answers, text from your files, and any secret that passed through a run, such as an API key.
      <br /><br />
      They're sealed with a key made on this computer and kept in your system keychain, never in Chummy's folder.
      <ul>
        <li>Lose the key (a new computer, a reset keychain) and the logs can't be read by anyone, including you.</li>
        <li>Export the key (Settings → Logs, or the main menu) to back it up or read logs elsewhere. Anyone with the file and its passphrase can read them.</li>
        <li>Anything that can use your unlocked keychain as you can read them too.</li>
        <li>You can write logs unencrypted, shorten them, or turn them off in Settings → Logs.</li>
      </ul>
    </span>
  {/if}
</span>

<style>
  .tip-wrap {
    position: relative;
    display: inline-flex;
    vertical-align: middle;
  }
  .tip-btn {
    display: inline-grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--text-faint);
    cursor: help;
  }
  .tip-btn:hover,
  .tip-btn:focus-visible {
    color: var(--accent);
  }
  .tip {
    position: absolute;
    z-index: 50;
    bottom: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    width: 340px;
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
    box-shadow: var(--shadow-md, var(--shadow-sm));
    color: var(--text);
    font-size: var(--text-xs);
    font-weight: 400;
    line-height: 1.5;
    text-align: left;
    white-space: normal;
  }
  .tip.below {
    bottom: auto;
    top: calc(100% + 6px);
  }
  .tip ul {
    margin: var(--space-2) 0 0;
    padding-left: 1.2em;
  }
  .tip li {
    margin: 2px 0;
  }
</style>
