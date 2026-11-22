<script lang="ts">
  import SideNav from "$lib/components/SideNav.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import ThemeSwitch from "$lib/components/ThemeSwitch.svelte";
  import Button from "$lib/components/Button.svelte";

  type Section = "models" | "workspace" | "permissions" | "appearance" | "about";

  const sections: { id: Section; label: string; icon: "bot" | "folder" | "shield" | "palette" | "info" }[] = [
    { id: "models", label: "Models", icon: "bot" },
    { id: "workspace", label: "Workspace", icon: "folder" },
    { id: "permissions", label: "Permissions", icon: "shield" },
    { id: "appearance", label: "Appearance", icon: "palette" },
    { id: "about", label: "About", icon: "info" },
  ];

  let active = $state<Section>("models");
  const current = $derived(sections.find((s) => s.id === active)!);
</script>

<div class="layout">
  <aside class="side">
    <div class="side-title">Settings</div>
    <SideNav items={sections} bind:active label="Settings sections" />
  </aside>

  <main class="content">
    <h1>{current.label}</h1>

    {#if active === "models"}
      <p class="sub">The LLM endpoints your agents can use.</p>
      <div class="panel">
        <EmptyState icon="bot" title="No models connected">
          Add Ollama, an OpenAI-compatible server, OpenAI, Anthropic or Gemini.
          {#snippet actions()}
            <Button variant="primary" icon="plus" disabled>Add a model</Button>
          {/snippet}
        </EmptyState>
      </div>
    {:else if active === "workspace"}
      <p class="sub">The one folder agents are allowed to see. Nothing outside it is visible to them.</p>
      <div class="panel">
        <EmptyState icon="folder" title="No folder chosen">
          You'll pick and confirm a folder before any agent runs.
          {#snippet actions()}
            <Button icon="folder" disabled>Choose folder…</Button>
          {/snippet}
        </EmptyState>
      </div>
    {:else if active === "permissions"}
      <p class="sub">Everything beyond the workspace folder is off until you turn it on.</p>
      <div class="rows">
        <div class="row">
          <div><div class="row-title">Internet access</div><div class="row-sub">Let agents and tools reach the network.</div></div>
          <span class="pill">Off</span>
        </div>
        <div class="row">
          <div><div class="row-title">Tools</div><div class="row-sub">Let agents run installed tools.</div></div>
          <span class="pill">Off</span>
        </div>
      </div>
    {:else if active === "appearance"}
      <p class="sub">How Chummy looks. Changes apply to every window.</p>
      <div class="rows">
        <div class="row">
          <div><div class="row-title">Theme</div><div class="row-sub">Follow the system, or pick one.</div></div>
          <ThemeSwitch />
        </div>
      </div>
    {:else}
      <p class="sub">Chummy 0.1.0 · MIT license · Friendly Labs</p>
    {/if}
  </main>
</div>

<style>
  .layout {
    display: flex;
    height: 100%;
  }
  .side {
    flex: none;
    width: 212px;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }
  .side-title {
    padding: var(--space-5) var(--space-5) 0;
    font-weight: 650;
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: var(--space-6) var(--space-6);
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .sub {
    margin-top: var(--space-1);
    margin-bottom: var(--space-5);
    color: var(--text-muted);
  }
  .panel {
    display: flex;
    border-radius: var(--radius-lg);
    border: 1px dashed var(--border-strong);
  }
  .rows {
    border-radius: var(--radius-lg);
    border: 1px solid var(--border);
    background: var(--surface);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-4);
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .row-title {
    font-weight: 550;
  }
  .row-sub {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .pill {
    padding: 2px var(--space-3);
    border-radius: var(--radius-full);
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 600;
  }
</style>
