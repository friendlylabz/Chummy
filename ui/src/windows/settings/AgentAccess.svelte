<script lang="ts">
  /**
   * An agent's tool switchboard: for each installed tool, whether the agent
   * may use it, whether every use asks first, and whether it may go online.
   * The master switches stay in Security and Privacy; a loop block can only
   * switch a tool off for itself.
   *
   * For a template agent (one being created) nothing is saved here: the
   * switches are kept in `value` and saved, with their confirmations, when
   * the agent is created.
   */
  import { onMount } from "svelte";
  import { commands, errorText, type Agent, type AgentTool, type ToolView } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { tools } from "$lib/stores/tools.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Toggle from "$lib/components/Toggle.svelte";

  let {
    agent,
    gotoTools,
    gotoSecurity,
    template = false,
    value = $bindable({}),
  }: {
    agent: Agent;
    gotoTools: () => void;
    gotoSecurity: () => void;
    /** A template agent: keep the switches in `value` instead of saving them. */
    template?: boolean;
    value?: Record<string, AgentTool>;
  } = $props();

  let error = $state("");
  let busy = $state(false);

  onMount(() => tools.init());

  const perms = $derived(settings.current.permissions);
  const mine = $derived<Record<string, AgentTool>>(template ? value : (agent.tools ?? {}));
  const ready = $derived(tools.ready);
  const name = $derived(agent.name.trim() || "this agent");

  async function apply(next: Record<string, AgentTool>) {
    error = "";
    if (template) {
      value = next;
      return;
    }
    busy = true;
    try {
      await commands.setAgentAccess(agent.id, next);
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  function setOn(t: ToolView, on: boolean) {
    const next = { ...mine };
    // A potentially unsafe tool starts out asking before each use.
    if (on) next[t.name] = { ask: t.risky, network: false };
    else delete next[t.name];
    apply(next);
  }

  function set(t: ToolView, key: keyof AgentTool, v: boolean) {
    const cur = mine[t.name];
    if (!cur) return;
    apply({ ...mine, [t.name]: { ...cur, [key]: v } });
  }

  /** Its programs get a network of their own when its Internet switch is on. */
  const direct = (t: ToolView) => t.needs.includes("direct-network");

  const onCount = $derived(Object.keys(mine).filter((n) => tools.installed.some((t) => t.name === n)).length);
</script>

<div class="card card-pad" class:dim={!ready || !perms.tools}>
  <div class="head">
    <div class="section-title"><Icon name="wrench" size={16} /> Tools for {name}</div>
    <span class="spacer"></span>
    {#if ready}<span class="muted small">{onCount ? `${onCount} on` : "None on"}</span>{/if}
  </div>
  <p class="explain">
    Switch on the tools {name} may use. <b>Ask first</b> makes every use wait for your OK in chat. <b>Internet</b> lets a
    tool that needs it go online. A loop block can switch a tool off for itself, but never on.
    {#if template}Chummy asks you to confirm anything risky when you create the agent.{/if}
  </p>

  {#if !tools.status}
    <p class="muted small">Loading…</p>
  {:else if !ready}
    <div class="notice">
      <Icon name="wrench" size={16} />
      <span style="flex: 1">Tools aren't set up yet. Set them up once, then switch them on for each agent here.</span>
      <Button variant="primary" onclick={gotoTools}>Set up tools</Button>
    </div>
  {:else if !perms.tools}
    <div class="notice warn">
      <Icon name="shield" size={16} />
      <span style="flex: 1">Tools are off for the whole app, so none of these run until you turn them on in Security and Privacy.</span>
      <Button variant="ghost" onclick={gotoSecurity}>Security and Privacy</Button>
    </div>
  {/if}

  <div class="board" role="table" aria-label="Tools for {name}">
    <div class="line top" role="row">
      <span role="columnheader">Tool</span>
      <span role="columnheader" class="c">On</span>
      <span role="columnheader" class="c">Ask first</span>
      <span role="columnheader" class="c" title={perms.network ? undefined : "Internet access is off in Security and Privacy"}>
        Internet{#if !perms.network} <Icon name="alert" size={11} />{/if}
      </span>
    </div>
    {#each tools.installed as t (t.name)}
      {@const cur = mine[t.name]}
      {@const asksAnyway = !!cur && t.network && cur.network && perms.network && perms.confirm_online_tools}
      <div class="line" role="row" class:unusable={!!t.unusable}>
        <span class="t-main" role="cell" title={t.unusable ?? undefined}>
          <span class="row-title">
            {t.title}
            {#if t.risky}<span class="risky" title="One call can change or delete many files in the workspace">potentially unsafe</span>{/if}
          </span>
          <span class="row-sub">{t.description}</span>
          {#if t.unusable}<span class="why"><Icon name="alert" size={12} /> {t.unusable}</span>{/if}
        </span>
        <span class="c" role="cell">
          <Toggle checked={!!cur} disabled={busy || (!!t.unusable && !cur)} label="{t.title} for {name}" onchange={(v) => setOn(t, v)} />
        </span>
        <span class="c" role="cell" title={asksAnyway ? "Asks anyway: it goes online, and online tools ask first (Security and Privacy)" : undefined}>
          <Toggle checked={!!cur && (cur.ask || asksAnyway)} disabled={busy || !cur || asksAnyway} label="Ask before each use of {t.title}" onchange={(v) => set(t, "ask", v)} />
        </span>
        <span class="c" role="cell">
          {#if t.network}
            <span title={direct(t) ? "Its programs get an ordinary network (Container sandbox): Chummy can't check where they connect" : undefined}>
              <Toggle checked={!!cur?.network} disabled={busy || !cur} label="Internet for {t.title}" onchange={(v) => set(t, "network", v)} />
            </span>
          {:else}
            <span class="faint xs" title="This tool doesn't use the internet">—</span>
          {/if}
        </span>
      </div>
    {:else}
      <div class="line empty"><span class="faint small">No tools installed yet.</span></div>
    {/each}
  </div>

  {#if ready && tools.installed.some((t) => t.network)}
    <p class="faint xs foot">
      {#if !perms.network}
        Internet access is off in Security and Privacy, so every tool stays offline.
      {:else}
        A tool that goes online does it through Chummy, which never lets it reach this computer or your local
        network{perms.confirm_online_tools ? ", and it asks first while “Ask before tools go online” is on" : ""}.
        {#if tools.installed.some(direct)}
          The exception is {tools.installed.filter(direct).map((t) => t.title).join(", ")}: in the Container sandbox
          its programs get an ordinary network that Chummy can't check.
        {/if}
      {/if}
    </p>
  {/if}
  {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
</div>

<style>
  .dim .board,
  .dim .foot {
    opacity: 0.55;
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .spacer {
    flex: 1;
  }
  .section-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 650;
  }
  .explain {
    margin: var(--space-2) 0 var(--space-3);
    color: var(--text-muted);
    font-size: var(--text-sm);
    line-height: 1.55;
  }
  .board {
    display: grid;
  }
  /* Every row uses the same columns, so the switches line up. */
  .line {
    display: grid;
    grid-template-columns: minmax(0, 1fr) repeat(3, 72px);
    align-items: center;
    column-gap: var(--space-2);
    padding: var(--space-2) 0;
  }
  .line + .line {
    border-top: 1px solid var(--border);
  }
  .line.top {
    padding-top: 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .line.empty {
    grid-template-columns: 1fr;
  }
  .c {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 3px;
  }
  .t-main {
    min-width: 0;
    display: grid;
  }
  .row-title {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .unusable .t-main {
    opacity: 0.55;
  }
  .why {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 2px;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .risky {
    color: var(--warn);
    font-size: var(--text-xs);
    font-weight: normal;
  }
  .foot {
    margin-top: var(--space-2);
  }
  .notice {
    margin-bottom: var(--space-3);
  }
</style>
