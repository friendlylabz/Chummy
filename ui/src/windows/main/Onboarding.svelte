<script lang="ts">
  import { commands, errorText } from "$lib/ipc";
  import { setup } from "$lib/stores/setup.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import KeyTip from "$lib/components/KeyTip.svelte";

  const s = $derived(setup.status);
  // The first step that isn't done is the one to highlight.
  const next = $derived(!s.model_configured ? 1 : !s.loop_configured ? 2 : !s.agent_configured ? 3 : 0);
  const sandboxName = $derived(
    { native: "Native (WebAssembly)", container: "Container", openshell: "OpenShell" }[settings.current.sandbox.kind],
  );
  const perms = $derived(settings.current.permissions);
  const toolsOn = $derived(Object.values(settings.current.tools).filter((t) => t.enabled).length);
  let adding = $state(false);
  let error = $state("");

  async function recommended() {
    adding = true;
    error = "";
    try {
      await commands.addRecommendedLoop();
    } catch (e) {
      error = errorText(e);
    } finally {
      adding = false;
    }
  }
</script>

<section class="card" aria-labelledby="welcome-title">
  <div class="mark" aria-hidden="true"></div>
  <h1 id="welcome-title">Welcome to Chummy</h1>
  <p class="lede">Three things to set up before your first chat, and two you can look at now or later.</p>

  <ol class="steps">
    <li class:done={s.model_configured}>
      <span class="num">{#if s.model_configured}<Icon name="check" size={14} />{:else}1{/if}</span>
      <div class="text">
        <h2>Connect a model</h2>
        <p>Ollama, LM Studio, llama.cpp, vLLM, a cloud provider, or the built-in engine.</p>
      </div>
      <Button variant={next === 1 ? "primary" : "secondary"} onclick={() => commands.openWindow("settings", "models")}>
        {s.model_configured ? "Manage" : "Add a model"}
      </Button>
    </li>

    <li class:done={s.loop_configured}>
      <span class="num">{#if s.loop_configured}<Icon name="check" size={14} />{:else}2{/if}</span>
      <div class="text">
        <h2>Add a loop</h2>
        <p>
          How an agent thinks. The recommended <b>Chummy assistant</b> works with tools until the job is done, tells
          you what it's doing and keeps its own to-do list. Or build your own in the loop editor.
        </p>
        {#if error}<p class="err">{error}</p>{/if}
      </div>
      <div class="btns">
        {#if !s.loop_configured}
          <Button variant={next === 2 ? "primary" : "secondary"} disabled={adding} onclick={recommended}>
            Use recommended
          </Button>
        {/if}
        <Button variant={s.loop_configured ? "secondary" : "ghost"} icon="flow" onclick={() => commands.openWindow("editor")}>
          {s.loop_configured ? "Loop editor" : "Build my own"}
        </Button>
      </div>
    </li>

    <li class:done={s.agent_configured}>
      <span class="num">{#if s.agent_configured}<Icon name="check" size={14} />{:else}3{/if}</span>
      <div class="text">
        <h2>Set up an agent</h2>
        <p>Give a loop a model and its own workspace folder.</p>
      </div>
      <Button variant={next === 3 ? "primary" : "secondary"} onclick={() => commands.openWindow("settings", "agents")}>
        {s.agent_configured ? "Manage" : "New agent"}
      </Button>
    </li>
  </ol>

  <div class="optional-label">Optional</div>
  <ol class="steps">
    <li class="opt">
      <span class="num"><Icon name="shield" size={14} /></span>
      <div class="text">
        <h2>Security and Privacy</h2>
        <p>
          Switch tools and internet access on for the whole app, and choose where tools run.
          <span class="now">Now: tools {perms.tools ? "on" : "off"}, internet {perms.network ? "on" : "off"}, sandbox {sandboxName}.</span>
        </p>
      </div>
      <Button variant="ghost" onclick={() => commands.openWindow("settings", "security")}>Open</Button>
    </li>
    <li class="opt">
      <span class="num"><Icon name="wrench" size={14} /></span>
      <div class="text">
        <h2>Tools</h2>
        <p>
          Set up the tools agents can call: reading and editing files, searching, commands, the web.
          <span class="now">Now: {toolsOn ? `${toolsOn} switched on` : "none switched on"}.</span>
        </p>
      </div>
      <Button variant="ghost" onclick={() => commands.openWindow("settings", "tools")}>Open</Button>
    </li>
  </ol>

  <p class="note">
    <Icon name="shield" size={14} />
    Each agent only ever sees its own folder, and stays offline unless you allow it.
  </p>
  <p class="note">
    <Icon name="key" size={14} />
    Every run is logged, encrypted with a key in your keychain.
    <KeyTip />
  </p>
</section>

<style>
  .btns {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    align-items: stretch;
  }
  .err {
    color: var(--danger);
  }
  .card {
    width: min(540px, 100%);
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
    background: linear-gradient(135deg, #3e9c76, #2b784e);
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
    padding: var(--space-3);
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
  .optional-label {
    margin: var(--space-4) 0 0;
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .optional-label + .steps {
    margin-top: var(--space-2);
  }
  .opt {
    background: transparent !important;
    border-style: dashed !important;
  }
  .opt .num {
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .now {
    display: block;
    color: var(--text-faint);
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
