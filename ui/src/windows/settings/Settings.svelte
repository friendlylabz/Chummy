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

  type Section = "agents" | "models" | "engine" | "tools" | "permissions" | "general" | "appearance" | "about";

  const sections: {
    id: Section;
    label: string;
    icon: "bot" | "cpu" | "shield" | "palette" | "info" | "chat" | "wrench" | "sliders";
  }[] = [
    { id: "agents", label: "Agents", icon: "chat" },
    { id: "models", label: "Models", icon: "bot" },
    { id: "engine", label: "Local engine", icon: "cpu" },
    { id: "tools", label: "Tools", icon: "wrench" },
    { id: "permissions", label: "Permissions", icon: "shield" },
    { id: "general", label: "General", icon: "sliders" },
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
  const general = $derived(settings.current.general);
  const closeOptions = [
    { value: "ask", label: "Ask each time" },
    { value: "tray", label: "Keep running in the tray" },
    { value: "quit", label: "Quit" },
  ] as const;

  function setGeneral(patch: Partial<typeof general>) {
    commands.setGeneral({ ...general, ...patch });
  }

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
        <AgentsPane gotoModels={() => go("models", "pick")} gotoTools={() => go("tools")} />
      {:else if active === "tools"}
        <ToolsPane gotoPermissions={() => go("permissions")} />
      {:else if active === "permissions"}
        <h1>Permissions</h1>
        <p class="pane-sub">Everything beyond the workspace folder is off until you turn it on. Turning something on asks you to confirm.</p>
        <div class="card rows">
          <div class="row">
            <div>
              <div class="row-title">Internet access</div>
              <div class="row-sub">The app-wide switch. A tool goes online only if this, the tool's internet switch in Tools, the agent's internet switch in Agents, and the block's “may use the internet” are all on.</div>
            </div>
            <Toggle checked={perms.network} label="Internet access" onchange={(v) => commands.setPermission("network", v)} />
          </div>
          <div class="row">
            <div>
              <div class="row-title">Tools</div>
              <div class="row-sub">The app-wide switch. A tool runs only if this and the tool are on in Tools, the agent may use it (Agents), and the loop block ticks it. Tools stay locked to the agent's workspace.</div>
            </div>
            <Toggle checked={perms.tools} label="Tools" onchange={(v) => commands.setPermission("tools", v)} />
          </div>
          <div class="row">
            <div>
              <div class="row-title">Ask before tools go online</div>
              <div class="row-sub">
                Every call to a tool that can use the internet waits for your OK in chat, so a web page can't quietly
                talk an agent into sending your files somewhere. Scheduled jobs wait too.
              </div>
            </div>
            <Toggle
              checked={perms.confirm_online_tools}
              label="Ask before tools go online"
              onchange={(v) => commands.setPermission("confirm_online_tools", v)}
            />
          </div>
        </div>
        <p class="faint xs" style="margin-top: var(--space-3)">
          Model connections in Models are made by Chummy itself and work regardless of these switches.
        </p>
      {:else if active === "general"}
        <h1>General</h1>
        <p class="pane-sub">Closing Chummy, notifications and scheduled jobs.</p>
        <div class="card rows">
          <div class="row">
            <div>
              <div class="row-title">When I close the main window</div>
              <div class="row-sub">
                In the tray, Chummy keeps running scheduled jobs; its icon shows it's there. Quitting always asks first while
                something is running.
              </div>
            </div>
            <select
              value={general.close}
              aria-label="When I close the main window"
              onchange={(e) => setGeneral({ close: e.currentTarget.value as typeof general.close })}
            >
              {#each closeOptions as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
            </select>
          </div>
          <div class="row">
            <div>
              <div class="row-title">Notifications</div>
              <div class="row-sub">When an agent asks you something while Chummy isn't in front, and when jobs finish.</div>
            </div>
            <Toggle checked={general.notifications} label="Notifications" onchange={(v) => setGeneral({ notifications: v })} />
          </div>
          <div class="row">
            <div>
              <div class="row-title">Scheduled runs at once</div>
              <div class="row-sub">An agent still does one thing at a time. Keep this at 1 with a local model.</div>
            </div>
            <select
              value={String(general.max_job_runs)}
              aria-label="Scheduled runs at once"
              onchange={(e) => setGeneral({ max_job_runs: Number(e.currentTarget.value) })}
            >
              {#each [1, 2, 3, 4] as n (n)}<option value={String(n)}>{n}</option>{/each}
            </select>
          </div>
        </div>
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
