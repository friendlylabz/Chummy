<script lang="ts">
  import { onMount } from "svelte";
  import { commands, events, type AppInfo } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { local } from "$lib/stores/local.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import SideNav from "$lib/components/SideNav.svelte";
  import ThemeSwitch from "$lib/components/ThemeSwitch.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import ModelsPane from "./ModelsPane.svelte";
  import LocalEnginePane from "./LocalEnginePane.svelte";
  import AgentsPane from "./AgentsPane.svelte";
  import ToolsPane from "./ToolsPane.svelte";
  import LogsPane from "./LogsPane.svelte";
  import SecurityPane from "./SecurityPane.svelte";

  type Section = "agents" | "models" | "engine" | "tools" | "security" | "logs" | "general" | "appearance" | "about";

  const sections: {
    id: Section;
    label: string;
    icon: "bot" | "cpu" | "shield" | "palette" | "info" | "chat" | "wrench" | "sliders" | "history";
  }[] = [
    { id: "agents", label: "Agents", icon: "chat" },
    { id: "models", label: "Models", icon: "bot" },
    { id: "engine", label: "Local engine", icon: "cpu" },
    { id: "tools", label: "Tools", icon: "wrench" },
    { id: "security", label: "Security and Privacy", icon: "shield" },
    { id: "logs", label: "Logs", icon: "history" },
    { id: "general", label: "General", icon: "sliders" },
    { id: "appearance", label: "Appearance", icon: "palette" },
    { id: "about", label: "About", icon: "info" },
  ];

  const fromHash = (h: string): Section | null => {
    let id = h.replace(/^#/, "").split(":")[0];
    // The section was called Permissions before.
    if (id === "permissions") id = "security";
    return sections.some((x) => x.id === id) ? (id as Section) : null;
  };
  /** `logs:export` and `logs:import` open those dialogs. */
  const logsAction = (h: string) => {
    const a = h.replace(/^#/, "").split(":")[1];
    return a === "export" || a === "import" ? a : null;
  };
  let active = $state<Section>(fromHash(location.hash) ?? "models");
  let logsStart = $state<"export" | "import" | null>(logsAction(location.hash));
  /** Re-mounts the Logs pane when asked to open a dialog again. */
  let logsKey = $state(0);
  let modelsView = $state<"list" | "pick" | "form" | "local">("list");
  let info = $state<AppInfo | null>(null);
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
      if (s) {
        logsStart = logsAction(section);
        logsKey++;
        go(s);
      }
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
        <AgentsPane gotoModels={() => go("models", "pick")} gotoTools={() => go("tools")} gotoSecurity={() => go("security")} />
      {:else if active === "tools"}
        <ToolsPane gotoSecurity={() => go("security")} gotoAgents={() => go("agents")} />
      {:else if active === "security"}
        <SecurityPane gotoTools={() => go("tools")} />
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
              <div class="row-sub">Keep this at 1 with a local model.</div>
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
      {:else if active === "logs"}
        {#key logsKey}<LogsPane start={logsStart} />{/key}
      {:else if active === "appearance"}
        <h1>Appearance</h1>
        <p class="pane-sub">How Chummy looks. Changes apply to every window.</p>
        <div class="card rows">
          <div class="row">
            <div><div class="row-title">Theme</div><div class="row-sub">Follow the system, or pick one.</div></div>
            <ThemeSwitch />
          </div>
        </div>
        <div class="section-label">Replies in chat</div>
        <div class="card rows">
          <div class="row">
            <div>
              <div class="row-title">Format replies</div>
              <div class="row-sub">Show Markdown as formatting: headings, lists, tables, quotes, code blocks, bold and links. Off shows the text exactly as the model wrote it.</div>
            </div>
            <Toggle checked={general.markdown} label="Format replies" onchange={(v) => setGeneral({ markdown: v })} />
          </div>
          <div class="row">
            <div>
              <div class="row-title">Show math</div>
              <div class="row-sub">Typeset LaTeX between $…$, $$…$$, \(…\) or \[…\]. Amounts like “$5 and $10” are left alone.</div>
            </div>
            <Toggle checked={general.math} label="Show math" onchange={(v) => setGeneral({ math: v })} />
          </div>
        </div>
      {:else}
        <h1>About</h1>
        <p class="pane-sub">Chummy {info?.version ?? ""} · MIT license · friendlylabz</p>
        <p class="muted small">
          Where Chummy keeps its settings, loops and history (and how it's protected) is in
          <button type="button" class="link" onclick={() => go("security")}>Security and Privacy</button>.
        </p>
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
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
</style>
