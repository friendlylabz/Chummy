<script lang="ts">
  import { onMount } from "svelte";
  import { commands, events, type AppInfo } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { local } from "$lib/stores/local.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import SideNav from "$lib/components/SideNav.svelte";
  import ThemeSwitch from "$lib/components/ThemeSwitch.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import ModelsPane from "./ModelsPane.svelte";
  import LocalEnginePane from "./LocalEnginePane.svelte";
  import AgentsPane from "./AgentsPane.svelte";
  import ToolsPane from "./ToolsPane.svelte";

  type Section = "agents" | "models" | "engine" | "tools" | "permissions" | "appearance" | "about";

  const sections: { id: Section; label: string; icon: "bot" | "cpu" | "shield" | "palette" | "info" | "chat" | "wrench" }[] = [
    { id: "agents", label: "Agents", icon: "chat" },
    { id: "models", label: "Models", icon: "bot" },
    { id: "engine", label: "Local engine", icon: "cpu" },
    { id: "tools", label: "Tools", icon: "wrench" },
    { id: "permissions", label: "Permissions", icon: "shield" },
    { id: "appearance", label: "Appearance", icon: "palette" },
    { id: "about", label: "About", icon: "info" },
  ];

  const fromHash = (h: string): Section | null => {
    const id = h.replace(/^#/, "");
    return sections.some((x) => x.id === id) ? (id as Section) : null;
  };
  let active = $state<Section>(fromHash(location.hash) ?? "models");
  let modelsView = $state<"list" | "pick" | "form" | "local">("list");
  let info = $state<AppInfo | null>(null);
  const perms = $derived(settings.current.permissions);

  onMount(async () => {
    local.init();
    loops.init();
    events.onGoto((section) => {
      const s = fromHash(section);
      if (s) go(s);
    });
    info = await commands.appInfo();
  });

  function go(section: Section, view: typeof modelsView = "list") {
    active = section;
    modelsView = view;
  }
</script>

<div class="layout">
  <aside class="side">
    <div class="side-title">Settings</div>
    <SideNav items={sections} active={active} onselect={(id) => go(id)} label="Settings sections" />
  </aside>

  <main class="content">
    <div class="inner">
      {#if info?.recovery}
        <div class="notice warn" style="margin-bottom: var(--space-4)">
          <Icon name="alert" size={16} />
          <span>
            {info.recovery.kind === "used_backup"
              ? "Your settings file was damaged, so Chummy restored the previous version."
              : "Your settings file was damaged and no backup was usable, so Chummy started fresh."}
            The damaged copy was kept in the Chummy folder.
          </span>
        </div>
      {/if}

      {#if active === "models"}
        <ModelsPane bind:view={modelsView} gotoEngine={() => go("engine")} />
      {:else if active === "engine"}
        <LocalEnginePane addModel={() => go("models", "local")} />
      {:else if active === "agents"}
        <AgentsPane gotoModels={() => go("models", "pick")} />
      {:else if active === "tools"}
        <ToolsPane gotoPermissions={() => go("permissions")} />
      {:else if active === "permissions"}
        <h1>Permissions</h1>
        <p class="pane-sub">Everything beyond the workspace folder is off until you turn it on. Turning something on asks you to confirm.</p>
        <div class="card rows">
          <div class="row">
            <div>
              <div class="row-title">Internet access</div>
              <div class="row-sub">Let tools that you allow in Tools reach the internet. Off means no tool can go online.</div>
            </div>
            <Toggle checked={perms.network} label="Internet access" onchange={(v) => commands.setPermission("network", v)} />
          </div>
          <div class="row">
            <div>
              <div class="row-title">Tools</div>
              <div class="row-sub">Let agents run the tools you switch on in Tools. Tools stay locked to the agent's workspace.</div>
            </div>
            <Toggle checked={perms.tools} label="Tools" onchange={(v) => commands.setPermission("tools", v)} />
          </div>
        </div>
        <p class="faint xs" style="margin-top: var(--space-3)">
          Model connections in Models are made by Chummy itself and work regardless of these switches.
        </p>
      {:else if active === "appearance"}
        <h1>Appearance</h1>
        <p class="pane-sub">How Chummy looks. Changes apply to every window.</p>
        <div class="card rows">
          <div class="row">
            <div><div class="row-title">Theme</div><div class="row-sub">Follow the system, or pick one.</div></div>
            <ThemeSwitch />
          </div>
        </div>
      {:else}
        <h1>About</h1>
        <p class="pane-sub">Chummy {info?.version ?? ""} · MIT license · friendlylabz</p>
        <div class="card card-pad">
          <div class="row-title">Chummy folder</div>
          <p class="muted small" style="margin: 4px 0 var(--space-3)">
            Settings, saved loops, chat history, downloaded models and the local engine live here. Agents can never see it.
            API keys aren't here; they're in your system keychain.
          </p>
          <div class="mono small path" data-selectable>{info?.data_dir}</div>
          <div class="actions"><Button icon="external" onclick={() => commands.revealDataDir()}>Open folder</Button></div>
        </div>
      {/if}
    </div>
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
  }
  .inner {
    max-width: 760px;
    padding: var(--space-6);
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .path {
    word-break: break-all;
  }
</style>
