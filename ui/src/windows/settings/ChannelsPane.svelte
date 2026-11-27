<script lang="ts">
  /**
   * Settings → Channels: ways to talk to agents from outside Chummy, the
   * same channels chummyd has. Nothing here names a kind of channel: the
   * form is drawn from what each kind asks (its fields), and what a channel
   * asks to verify (emoji to compare, someone to let in…) shows the same
   * way for every kind. Secrets stay in the keychain, what a channel keeps
   * is sealed, so it connects again whenever Chummy starts.
   */
  import { onMount } from "svelte";
  import {
    commands,
    errorText,
    events,
    type ChannelConfig,
    type ChannelKindInfo,
    type ChannelView,
  } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import ChannelFields from "./ChannelFields.svelte";

  let { gotoAgents }: { gotoAgents: () => void } = $props();

  let kinds = $state.raw<ChannelKindInfo[]>([]);
  let list = $state<ChannelView[]>([]);
  let editing = $state<ChannelView | "new" | null>(null);
  let error = $state("");
  let busy = $state(false);
  /** Now, for how long invitations still work (ticks while one shows). */
  let now = $state(Date.now());

  // The form.
  let type = $state("");
  let name = $state("");
  let chosen = $state<string[]>([]);
  let approvals = $state(false);
  let options = $state<Record<string, unknown>>({});
  let secrets = $state<Record<string, string>>({});

  const agents = $derived(settings.current.agents);
  const agentName = (id: string) => agents.find((a) => a.id === id)?.name ?? "Missing agent";
  const kindOf = (t: string) => kinds.find((k) => k.id === t);
  const kind = $derived(kindOf(type));
  const names = (ids: string[]) => {
    const n = ids.map(agentName);
    return n.length > 1 ? `${n.slice(0, -1).join(", ")} and ${n[n.length - 1]}` : (n[0] ?? "");
  };

  async function load() {
    now = Date.now();
    try {
      list = await commands.listChannels();
    } catch (e) {
      error = errorText(e);
    }
  }

  onMount(() => {
    commands.channelKinds().then((k) => (kinds = k));
    load();
    const offs = [events.onChannelsChanged(() => load()), events.onSettingsChanged(() => load())];
    const tick = setInterval(() => (now = Date.now()), 15000);
    return () => {
      clearInterval(tick);
      offs.forEach((o) => o.then((f) => f()));
    };
  });

  /** A kind's fields with their defaults. */
  function defaults(k: ChannelKindInfo | undefined): Record<string, unknown> {
    const o: Record<string, unknown> = {};
    for (const f of k?.fields ?? []) if (f.input !== "secret") o[f.key] = $state.snapshot(f.default);
    return o;
  }

  function setType(t: string) {
    if (editing !== "new") return;
    if (!name || name === kindOf(type)?.label) name = kindOf(t)?.label ?? "";
    type = t;
    options = defaults(kindOf(t));
    secrets = {};
    if (!kindOf(t)?.many_agents) chosen = chosen.slice(0, 1);
  }

  function open(c: ChannelView | "new") {
    editing = c;
    error = "";
    secrets = {};
    if (c === "new") {
      type = "";
      name = "";
      chosen = [settings.current.active_agent ?? agents[0]?.id ?? ""].filter(Boolean);
      approvals = false;
      setType(kinds[0]?.id ?? "");
    } else {
      type = c.type;
      name = c.name;
      chosen = [...c.agents];
      approvals = c.approvals;
      options = { ...defaults(kindOf(c.type)), ...$state.snapshot(c.options) };
    }
  }

  function toggleAgent(id: string, on: boolean) {
    if (!kind?.many_agents) chosen = on ? [id] : [];
    else chosen = on ? [...chosen.filter((a) => a !== id), id] : chosen.filter((a) => a !== id);
  }

  const ready = $derived(!!name.trim() && chosen.length > 0 && !!kind);

  async function save() {
    busy = true;
    error = "";
    const old = editing !== "new" ? editing : null;
    const channel: ChannelConfig = {
      id: old?.id ?? "",
      name,
      enabled: old?.enabled ?? true,
      type,
      agents: chosen,
      approvals,
      options,
    };
    try {
      const id = await commands.saveChannel(channel, secrets);
      if (id) editing = null;
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  async function run(f: () => Promise<unknown>) {
    error = "";
    try {
      await f();
    } catch (e) {
      error = errorText(e);
    }
  }

  const toggle = (c: ChannelView, on: boolean) => run(() => commands.setChannelEnabled(c.id, on));
  const remove = (c: ChannelView) => run(() => commands.deleteChannel(c.id));
  const answer = (c: ChannelView, n: number, yes: boolean) => run(() => commands.answerChannelPending(c.id, n, yes));
  const invite = (c: ChannelView) => run(() => commands.channelInvite(c.id));
  const removePerson = (c: ChannelView, id: string) => run(() => commands.removeChannelPerson(c.id, id));

  const stateLabel: Record<string, string> = {
    connected: "Connected",
    connecting: "Connecting…",
    problem: "Not connected",
    off: "Off",
    "": "Starting…",
  };
</script>

{#if editing}
  <button class="back" type="button" onclick={() => (editing = null)}><Icon name="chevron-left" size={16} />Channels</button>
  <h1>{editing === "new" ? "New channel" : `Edit ${editing.name}`}</h1>
  <p class="pane-sub">Agents, and the people who may talk to them from outside Chummy.</p>

  <div class="card card-pad">
    <div class="field">
      <label for="ch-type">Type</label>
      <select
        id="ch-type"
        class="input"
        value={type}
        onchange={(e) => setType(e.currentTarget.value)}
        disabled={editing !== "new"}
      >
        {#each kinds as k (k.id)}<option value={k.id}>{k.label}</option>{/each}
      </select>
      {#if kind}<span class="hint">{kind.about}</span>{/if}
    </div>

    <div class="field">
      <label for="ch-name">Name</label>
      <input id="ch-name" class="input" bind:value={name} maxlength="60" placeholder="e.g. Phone" />
    </div>

    <div class="field">
      <span class="label">{kind?.many_agents ? "Agents" : "Agent"}</span>
      {#if agents.length}
        {#if kind?.many_agents}
          <div class="agents">
            {#each agents as a (a.id)}
              <label class="agent-pick">
                <input type="checkbox" checked={chosen.includes(a.id)} onchange={(e) => toggleAgent(a.id, e.currentTarget.checked)} />
                {a.name}
              </label>
            {/each}
          </div>
        {:else}
          <select class="input" aria-label="Agent" value={chosen[0] ?? ""} onchange={(e) => toggleAgent(e.currentTarget.value, true)}>
            {#each agents as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
          </select>
        {/if}
        <span class="hint">Every message starts a run of the agent's loop, with its tools, in its folder.</span>
      {:else}
        <div class="notice">
          <Icon name="bot" size={16} />
          <span style="flex: 1">There are no agents yet.</span>
          <Button onclick={gotoAgents}>Add an agent</Button>
        </div>
      {/if}
    </div>

    {#if kind}
      <h2 class="group">{kind.label}</h2>
      <ChannelFields
        {kind}
        bind:options
        bind:secrets
        saved={editing !== "new" ? editing.saved_secrets : []}
      />
    {/if}

    <div class="row toggle-row">
      <div>
        <div class="row-title">Answer tool approvals in {kind?.label ?? "the channel"}</div>
        <div class="row-sub">
          Off: approvals wait for you here, in Chummy, and the chat is told. On: they can be answered in the channel, so
          whoever can talk to the agent there can let it use its tools.
        </div>
      </div>
      <Toggle checked={approvals} label={`Answer tool approvals in ${kind?.label ?? "the channel"}`} onchange={(v) => (approvals = v)} />
    </div>

    {#if error}<div class="notice danger" style="margin-top: var(--space-4)"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}

    <div class="actions">
      <span class="spacer"></span>
      <Button variant="ghost" onclick={() => (editing = null)}>Cancel</Button>
      <Button variant="primary" onclick={save} disabled={busy || !ready}>
        {editing === "new" ? "Add channel" : "Save"}
      </Button>
    </div>
  </div>
{:else}
  <header class="pane-head">
    <div>
      <h1>Channels</h1>
      <p class="pane-sub">Talk to your agents from outside Chummy, like your phone, while Chummy runs.</p>
    </div>
    {#if list.length}<Button variant="primary" icon="plus" onclick={() => open("new")}>New channel</Button>{/if}
  </header>

  {#if error}<div class="notice danger" style="margin-bottom: var(--space-4)"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}

  {#if !list.length}
    <div class="card empty">
      <EmptyState icon="channel" title="No channels yet">
        Add one to talk to an agent from {kinds.map((k) => k.label).join(" or ") || "outside"}: messages start runs of its
        loop, and its replies come back as it works. Runs show in the sessions panel like any other.
        {#snippet actions()}
          <Button variant="primary" icon="plus" onclick={() => open("new")} disabled={!agents.length || !kinds.length}>New channel</Button>
        {/snippet}
      </EmptyState>
    </div>
  {/if}

  {#each list as c (c.id)}
    {@const st = c.enabled ? c.live.state : "off"}
    {@const k = kindOf(c.type)}
    {@const count = c.people.length}
    <div class="card card-pad channel">
      <div class="c-head">
        <div class="c-main">
          <div class="c-name">
            {c.name}
            <span class="badge">{k?.label ?? c.type}</span>
            <span class="status" class:ok={st === "connected"} class:bad={st === "problem"}>{stateLabel[st] ?? st}</span>
          </div>
          <div class="muted small">
            <Icon name="bot" size={12} /> {names(c.agents)} · {count} {count === 1 ? "person" : "people"}
            {#if c.approvals}· approvals in {k?.label ?? "it"}{/if}
          </div>
          <div class="faint xs">
            {#if c.live.account}<span class="mono">{c.live.account}</span>{" · "}{/if}{c.summary}
          </div>
          {#if c.enabled && c.live.message}<div class="xs problem">{c.live.message}</div>{/if}
        </div>
        <div class="c-actions">
          <Toggle checked={c.enabled} label="{c.name} on" onchange={(v) => toggle(c, v)} />
          <Button variant="ghost" icon="edit" iconOnly aria-label="Edit" onclick={() => open(c)} />
          <Button variant="ghost" icon="trash" iconOnly aria-label="Delete" onclick={() => remove(c)} />
        </div>
      </div>

      <!-- What the channel asks to verify: the same for every kind. -->
      {#each c.live.pending as v (v.n)}
        <div class="verify">
          <div class="small"><b>{v.who}</b> {v.ask}</div>
          {#if v.items.length}
            <div class="emoji">
              {#each v.items as [symbol, label], i (i)}
                <span class="em"><span class="sym">{symbol}</span><span class="lab">{label}</span></span>
              {/each}
            </div>
          {/if}
          <div class="v-actions">
            <Button variant="primary" onclick={() => answer(c, v.n, true)}>{v.yes}</Button>
            <Button variant="ghost" onclick={() => answer(c, v.n, false)}>{v.no}</Button>
          </div>
        </div>
      {/each}

      {#if k?.invites || (k?.removes_people && c.people.length)}
        <div class="people">
          {#each c.people as p (p.id)}
            <div class="person">
              <Icon name="person" size={13} />
              <span class="small">{p.name}</span>
              {#if p.name !== p.id}<span class="faint xs mono">{p.id}</span>{/if}
              <span class="spacer"></span>
              {#if k?.removes_people}<Button variant="ghost" onclick={() => removePerson(c, p.id)}>Remove</Button>{/if}
            </div>
          {/each}
          {#if c.live.invite && c.live.invite.expires_at > now}
            <div class="code">
              <div class="code-num mono">{c.live.invite.code}</div>
              <div class="small">{c.live.invite.text}</div>
            </div>
          {/if}
          {#if k?.invites}
            <div class="p-actions">
              <Button
                variant={c.people.length ? "ghost" : "primary"}
                icon="plus"
                disabled={!c.enabled || st !== "connected"}
                onclick={() => invite(c)}
              >
                {c.live.invite && c.live.invite.expires_at > now ? "New code" : k.invites}
              </Button>
              {#if !c.enabled || st !== "connected"}<span class="faint xs">The channel has to be connected for that.</span>{/if}
            </div>
          {/if}
        </div>
      {/if}
      {#if c.live.note}<div class="faint xs note">{c.live.note}</div>{/if}
    </div>
  {/each}

  {#if list.length}
    <p class="faint xs" style="margin-top: var(--space-4)">
      In a chat: send a message to talk to the agent, reply to its questions, and /help for the commands (/stop, /new,
      /status…). Chummy connects to the channel's servers itself, like model connections: the Internet permission
      doesn't apply.
    </p>
  {/if}
{/if}

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .pane-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-4);
  }
  .empty {
    display: flex;
    border-style: dashed;
    border-color: var(--border-strong);
    background: transparent;
  }
  .group {
    margin: var(--space-5) 0 var(--space-2);
    font-size: var(--text-sm);
    font-weight: 650;
  }
  .toggle-row {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    margin-top: var(--space-3);
  }
  .toggle-row > div {
    flex: 1;
  }
  .c-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .c-main {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .c-name {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: 600;
  }
  .c-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .badge {
    padding: 0 6px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 500;
  }
  .status {
    padding: 0 7px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 500;
  }
  .status.ok {
    background: color-mix(in srgb, var(--ok) 15%, transparent);
    color: var(--ok);
  }
  .status.bad {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .problem {
    color: var(--warn);
  }
  .verify {
    margin-top: var(--space-3);
    padding: var(--space-3);
    border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
    border-radius: var(--radius-md);
    background: var(--accent-soft);
  }
  .emoji {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-3);
    margin: var(--space-3) 0;
  }
  .em {
    display: grid;
    justify-items: center;
    min-width: 56px;
  }
  .sym {
    font-size: 28px;
    line-height: 1.2;
  }
  .lab {
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .v-actions {
    display: flex;
    gap: var(--space-2);
  }
  .note {
    margin-top: var(--space-2);
  }
  .agents {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-4);
  }
  .agent-pick {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-weight: 400;
  }
  .field .label {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .people {
    display: grid;
    gap: var(--space-1);
    margin-top: var(--space-3);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border);
  }
  .person {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .spacer {
    flex: 1;
  }
  .p-actions {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-top: var(--space-1);
  }
  .code {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-3);
    border-radius: var(--radius-md);
    background: var(--surface-2);
  }
  .code-num {
    font-size: 24px;
    font-weight: 650;
    letter-spacing: 0.12em;
  }
</style>
