<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, type ToolView } from "$lib/ipc";
  import { bytes } from "$lib/format";
  import { settings } from "$lib/stores/settings.svelte";
  import { tools } from "$lib/stores/tools.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";

  let { gotoPermissions }: { gotoPermissions: () => void } = $props();

  let error = $state("");
  let open = $state<string | null>(null);

  const s = $derived(tools.status);
  const perms = $derived(settings.current.permissions);
  const progress = $derived(tools.progress);

  onMount(() => tools.init());

  async function setup() {
    error = "";
    try {
      await commands.setupTools();
    } catch (e) {
      error = errorText(e);
    }
  }

  async function toggle(t: ToolView, on: boolean) {
    error = "";
    try {
      await commands.setToolEnabled(t.name, on);
    } catch (e) {
      error = errorText(e);
    }
  }

  async function ask(t: ToolView, on: boolean) {
    error = "";
    try {
      await commands.setToolAsk(t.name, on);
    } catch (e) {
      error = errorText(e);
    }
  }

  async function net(t: ToolView, on: boolean) {
    error = "";
    try {
      await commands.setToolNetwork(t.name, on);
    } catch (e) {
      error = errorText(e);
    }
  }

  function params(t: ToolView): string {
    const props = t.parameters.properties ?? {};
    const req = new Set(t.parameters.required ?? []);
    return Object.keys(props)
      .map((k) => (req.has(k) ? k : `${k}?`))
      .join(", ");
  }
</script>

<h1>Tools</h1>
<p class="pane-sub">
  Small Python programs agents can call. Each runs in a WebAssembly sandbox that can only see the agent's workspace
  folder, and can only go online through Chummy, when you allow it.
</p>

{#if error || tools.error}
  <div class="notice danger" role="alert" style="margin-bottom: var(--space-4)">
    <Icon name="x-circle" size={16} /><span>{error || tools.error}</span>
  </div>
{/if}

{#if !s}
  <p class="muted small">Loading…</p>
{:else if !s.runtime || (s.installing && !s.tools.length)}
  <div class="card card-pad">
    {#if s.installing || progress}
      <div class="row-title">Setting up tools…</div>
      <p class="muted small">{progress?.message || "Starting…"}</p>
      <div style="margin-top: var(--space-3)">
        <ProgressBar value={progress && progress.total ? progress.done / progress.total : null} label="Setting up tools" />
      </div>
      {#if progress?.total}<p class="faint xs" style="margin-top: var(--space-2)">{bytes(progress.done)} of {bytes(progress.total)}</p>{/if}
    {:else}
      <div class="row-title">Not set up yet</div>
      <p class="muted small" style="margin: 4px 0 var(--space-3)">
        Tools need Python {s.python_version} built for WebAssembly (about {bytes(s.download_bytes)}). Chummy downloads it
        once into its own folder, checks it, and installs its {s.builtin_count} built-in tools. Nothing is installed on
        your system.
      </p>
      <div class="actions"><Button variant="primary" icon="download" onclick={setup}>Set up tools</Button></div>
    {/if}
  </div>
{:else}
  {#if !perms.tools}
    <div class="notice warn" style="margin-bottom: var(--space-4)">
      <Icon name="shield" size={16} />
      <span>Tools are off in Permissions, so agents can't use any of these yet.</span>
      <Button variant="ghost" onclick={gotoPermissions}>Permissions</Button>
    </div>
  {/if}

  <p class="muted small" style="margin-bottom: var(--space-3)">
    Switching a tool on here makes it available. An agent uses it only once you also allow it for that agent (Agents)
    and tick it in the loop block that calls it.
  </p>
  <div class="section-label">Installed</div>
  <div class="card rows">
    {#each s.tools as t (t.name)}
      <div class="tool">
        <div class="row">
          <button type="button" class="tool-main" onclick={() => (open = open === t.name ? null : t.name)} aria-expanded={open === t.name}>
            <div class="row-title">
              {t.title}
              <span class="mono faint xs">{t.name}</span>
              {#if t.network}<span class="badge" title="Asks for internet access"><Icon name="globe" size={11} /> Internet</span>{/if}
              {#if t.risky}<span class="badge risky" title="One call can change or delete many files in the workspace"><Icon name="alert" size={11} /> Potentially unsafe</span>{/if}
            </div>
            <div class="row-sub">{t.description}</div>
          </button>
          <Toggle checked={t.enabled} label="Use {t.title}" onchange={(v) => toggle(t, v)} />
        </div>
        {#if t.network && t.enabled}
          <div class="row sub-row">
            <div>
              <div class="small">Internet access for this tool</div>
              <div class="faint xs">
                {#if !perms.network}Also needs Internet access in Permissions.{:else}Can't reach this computer or your local network.{/if}
                Each agent and block must allow the internet too.
              </div>
            </div>
            <Toggle checked={t.network_allowed} label="Internet for {t.title}" onchange={(v) => net(t, v)} />
          </div>
        {/if}
        {#if t.enabled}
          <div class="row sub-row">
            <div>
              <div class="small">Ask me before each use</div>
              <div class="faint xs">
                {#if t.network && t.network_allowed && perms.confirm_online_tools}Already asks, because it can go online.{:else}Each call waits until you allow it in chat.{/if}
              </div>
            </div>
            <Toggle checked={t.ask} label="Ask before using {t.title}" onchange={(v) => ask(t, v)} />
          </div>
        {/if}
        {#if open === t.name}
          <div class="details faint xs">
            <div><span class="k">Arguments</span> <span class="mono">{params(t) || "none"}</span></div>
            <div><span class="k">Version</span> {t.version} · <span class="k">Time limit</span> {t.timeout_secs} s</div>
            <div>
              <span class="k">Packages</span>
              {t.packages.length ? t.packages.map((p) => `${p.name} ${p.version}`).join(", ") : "none"}
            </div>
          </div>
        {/if}
      </div>
    {/each}
  </div>
  <p class="faint xs" style="margin-top: var(--space-3)">
    Python {s.runtime.version} for WebAssembly. A tool can read and write only the agent's own workspace, and only
    tools that ask for it can go online, through Chummy, if you allow it here and in Permissions.
  </p>
{/if}

<style>
  .tool + .tool {
    border-top: 1px solid var(--border);
  }
  .tool .row {
    border: none;
  }
  .tool-main {
    flex: 1;
    min-width: 0;
    padding: 0;
    border: none;
    background: none;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .badge.risky {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 6px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 500;
  }
  .sub-row {
    padding-top: 0 !important;
    padding-left: var(--space-6) !important;
  }
  .details {
    display: grid;
    gap: 3px;
    padding: 0 var(--space-4) var(--space-3);
  }
  .k {
    color: var(--text-muted);
    font-weight: 550;
  }
</style>
