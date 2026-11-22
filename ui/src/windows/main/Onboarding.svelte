<script lang="ts">
  import { commands } from "$lib/ipc";
  import { setup } from "$lib/stores/setup.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
</script>

<section class="card" aria-labelledby="welcome-title">
  <div class="mark" aria-hidden="true"></div>
  <h1 id="welcome-title">Welcome to Chummy</h1>
  <p class="lede">Two things to set up before your first chat.</p>

  <ol class="steps">
    <li class:done={setup.status.model_configured}>
      <span class="num">
        {#if setup.status.model_configured}<Icon name="check" size={14} />{:else}1{/if}
      </span>
      <div class="text">
        <h2>Connect a model</h2>
        <p>Ollama, any OpenAI-compatible server, OpenAI, Anthropic or Gemini.</p>
      </div>
      <Button
        variant={setup.status.model_configured ? "secondary" : "primary"}
        onclick={() => commands.openWindow("settings")}
      >
        Open settings
      </Button>
    </li>

    <li class:done={setup.status.loop_configured}>
      <span class="num">
        {#if setup.status.loop_configured}<Icon name="check" size={14} />{:else}2{/if}
      </span>
      <div class="text">
        <h2>Add an agent loop</h2>
        <p>Import a loop file or build your own, block by block.</p>
      </div>
      <Button icon="flow" onclick={() => commands.openWindow("editor")}>Loop editor</Button>
    </li>
  </ol>

  <p class="note">
    <Icon name="shield" size={14} />
    Agents only ever see the one folder you choose, and stay offline unless you allow it.
  </p>
</section>

<style>
  .card {
    width: min(520px, 100%);
    margin: auto;
    padding: var(--space-6) var(--space-6) var(--space-5);
    border-radius: var(--radius-xl);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-sm);
  }
  .mark {
    width: 36px;
    height: 36px;
    margin-bottom: var(--space-4);
    border-radius: 11px;
    background: linear-gradient(135deg, #f6b27a, #e9795a);
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.015em;
  }
  .lede {
    margin-top: var(--space-1);
    color: var(--text-muted);
  }

  .steps {
    list-style: none;
    margin: var(--space-5) 0 0;
    padding: 0;
    display: grid;
    gap: var(--space-2);
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-3) var(--space-3) var(--space-3);
    border-radius: var(--radius-lg);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .num {
    flex: none;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: var(--text-sm);
    font-weight: 650;
  }
  .done .num {
    background: color-mix(in srgb, var(--ok) 16%, transparent);
    color: var(--ok);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  h2 {
    font-size: var(--text-md);
    font-weight: 600;
  }
  .text p {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }

  .note {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-4);
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
</style>
