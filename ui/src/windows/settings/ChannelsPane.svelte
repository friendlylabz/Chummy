<script lang="ts">
  /**
   * Settings → Channels: ways to talk to an agent from outside Chummy
   * (Matrix and Telegram), the same channels chummyd has. Each channel is
   * one agent and the people who may talk to it; its secret (password or
   * bot token) stays in the keychain and what it keeps is sealed, so it
   * connects again whenever Chummy starts.
   */
  import { onMount } from "svelte";
  import { commands, errorText, events, type ChannelConfig, type ChannelView } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Toggle from "$lib/components/Toggle.svelte";

  let { gotoAgents }: { gotoAgents: () => void } = $props();

  /** The kinds of channel there are; each has one implementation, shared with chummyd. */
  const kinds = [
    { type: "matrix", label: "Matrix" },
    { type: "telegram", label: "Telegram" },
  ] as const;
  type Kind = (typeof kinds)[number]["type"];
  const kindLabel = (t: Kind) => kinds.find((k) => k.type === t)?.label ?? t;
  const TELEGRAM_API = "https://api.telegram.org";

  let list = $state<ChannelView[]>([]);
  let editing = $state<ChannelView | "new" | null>(null);
  let error = $state("");
  let busy = $state(false);
  /** Now, for how long codes still work (ticks while one shows). */
  let now = $state(Date.now());

  // The form.
  let type = $state<Kind>("matrix");
  let name = $state("");
  let agent = $state("");
  let homeserver = $state("");
  let userId = $state("");
  let password = $state("");
  let users = $state("");
  let uploadMb = $state(100);
  let approvals = $state(false);
  let apiUrl = $state(TELEGRAM_API);
  let stream = $state(true);

  const agents = $derived(settings.current.agents);
  const agentName = (id: string) => agents.find((a) => a.id === id)?.name ?? "Missing agent";

  async function load() {
    now = Date.now();
    try {
      list = await commands.listChannels();
    } catch (e) {
      error = errorText(e);
    }
  }

  onMount(() => {
    load();
    const offs = [events.onChannelsChanged(() => load()), events.onSettingsChanged(() => load())];
    const tick = setInterval(() => (now = Date.now()), 15000);
    return () => {
      clearInterval(tick);
      offs.forEach((o) => o.then((f) => f()));
    };
  });

  function setType(t: Kind) {
    if (editing !== "new") return;
    if (name === kindLabel(type) || !name) name = list.length ? "" : kindLabel(t);
    type = t;
    uploadMb = t === "telegram" ? 20 : 100;
  }

  function open(c: ChannelView | "new") {
    editing = c;
    error = "";
    password = "";
    if (c === "new") {
      type = "matrix";
      name = list.length ? "" : "Matrix";
      agent = settings.current.active_agent ?? agents[0]?.id ?? "";
      homeserver = "https://";
      userId = "";
      users = "";
      uploadMb = 100;
      approvals = false;
      apiUrl = TELEGRAM_API;
      stream = true;
    } else {
      type = c.type;
      name = c.name;
      agent = c.agent;
      approvals = c.approvals;
      uploadMb = c.upload_mb;
      if (c.type === "matrix") {
        homeserver = c.homeserver;
        userId = c.user_id;
        users = c.users.join("\n");
      } else {
        apiUrl = c.api_url;
        stream = c.stream;
      }
    }
  }

  const ready = $derived(
    !!name.trim() &&
      !!agent &&
      (type === "matrix" ? !!userId.trim() : !!password.trim() || (editing !== "new" && !!editing?.has_secret)),
  );

  async function save() {
    busy = true;
    error = "";
    const old = editing !== "new" ? editing : null;
    const base = { id: old?.id ?? "", name, enabled: old?.enabled ?? true, agent, approvals };
    const channel: ChannelConfig =
      type === "matrix"
        ? {
            ...base,
            type,
            homeserver,
            user_id: userId,
            users: users.split(/[\s,]+/).filter(Boolean),
            upload_mb: Number(uploadMb) || 100,
          }
        : { ...base, type, api_url: apiUrl, upload_mb: Number(uploadMb) || 20, stream };
    try {
      const id = await commands.saveChannel(channel, password || null);
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
  const makeCode = (c: ChannelView) => run(() => commands.channelPairingCode(c.id));
  const removePerson = (c: ChannelView, id: string) => run(() => commands.removeChannelPerson(c.id, id));

  function minutesLeft(ms: number): string {
    const m = Math.ceil((ms - now) / 60000);
    return m <= 1 ? "a minute" : `${m} minutes`;
  }

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
  <p class="pane-sub">One agent, and the people who may talk to it from outside Chummy.</p>

  <div class="card card-pad">
    <div class="field">
      <label for="ch-type">Type</label>
      <select
        id="ch-type"
        class="input"
        value={type}
        onchange={(e) => setType(e.currentTarget.value as Kind)}
        disabled={editing !== "new"}
      >
        {#each kinds as k (k.type)}<option value={k.type}>{k.label}</option>{/each}
      </select>
      <span class="hint">
        {#if type === "matrix"}
          End-to-end encrypted Matrix rooms: Chummy logs in as a bot account and makes a room with each person you allow.
          The same as chummyd's Matrix channel.
        {:else}
          A Telegram bot for the agent: people you let in talk to it in a private chat. Not end-to-end encrypted:
          Telegram's servers can read these chats. The same as chummyd's Telegram bots.
        {/if}
      </span>
    </div>

    <div class="field">
      <label for="ch-name">Name</label>
      <input id="ch-name" class="input" bind:value={name} maxlength="60" placeholder="e.g. Phone" />
    </div>

    <div class="field">
      <label for="ch-agent">Agent</label>
      {#if agents.length}
        <select id="ch-agent" class="input" bind:value={agent}>
          {#each agents as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
        </select>
        <span class="hint">Every message starts a run of this agent's loop, with its tools, in its folder.</span>
      {:else}
        <div class="notice">
          <Icon name="bot" size={16} />
          <span style="flex: 1">There are no agents yet.</span>
          <Button onclick={gotoAgents}>Add an agent</Button>
        </div>
      {/if}
    </div>

    {#if type === "matrix"}
      <h2 class="group">The bot's account</h2>
      <div class="field">
        <label for="ch-hs">Homeserver</label>
        <input id="ch-hs" class="input mono" bind:value={homeserver} spellcheck="false" placeholder="https://matrix.example.org" />
        <span class="hint">https, or plain http only on this computer or your local network.</span>
      </div>
      <div class="field">
        <label for="ch-user">Bot's Matrix ID</label>
        <input id="ch-user" class="input mono" bind:value={userId} spellcheck="false" placeholder="@chummy-bot:example.org" />
        <span class="hint">An account of its own: not yours, and not one a chummyd uses.</span>
      </div>
      <div class="field">
        <label for="ch-pw">Bot's password</label>
        <input
          id="ch-pw"
          class="input mono"
          type="password"
          autocomplete="off"
          bind:value={password}
          placeholder={editing !== "new" && editing.has_secret ? "Saved. Leave blank to keep it." : "The bot account's password"}
        />
        <span class="hint">
          Kept in your system keychain. Chummy uses it to log in once and to set up the bot's cross-signing; after that it
          keeps the session, sealed, and connects again whenever it starts.
        </span>
      </div>

      <h2 class="group">Who may talk to it</h2>
      <div class="field">
        <label for="ch-users">People (Matrix IDs, one per line)</label>
        <textarea id="ch-users" class="input mono" rows="3" bind:value={users} spellcheck="false" placeholder="@you:example.org"></textarea>
        <span class="hint">
          Each gets an encrypted room with the bot. Before they can talk to the agent, they verify the bot from their
          Matrix app, and you compare the emoji here.
        </span>
      </div>
    {:else}
      <h2 class="group">The bot</h2>
      <div class="field">
        <label for="ch-token">Bot token</label>
        <input
          id="ch-token"
          class="input mono"
          type="password"
          autocomplete="off"
          spellcheck="false"
          bind:value={password}
          placeholder={editing !== "new" && editing.has_secret ? "Saved. Leave blank to keep it." : "123456:ABC-def…"}
        />
        <span class="hint">
          Make a bot with @BotFather in Telegram (/newbot) and paste the token it gives. It's the bot's whole login:
          kept in your system keychain. Use a bot of its own, not one a chummyd uses.
        </span>
      </div>
      <div class="row toggle-row">
        <div>
          <div class="row-title">Show answers as they're written</div>
          <div class="row-sub">Off: each answer comes when it's done.</div>
        </div>
        <Toggle checked={stream} label="Show answers as they're written" onchange={(v) => (stream = v)} />
      </div>
      <div class="field" style="margin-top: var(--space-4)">
        <label for="ch-api">Bot API server</label>
        <input id="ch-api" class="input mono" bind:value={apiUrl} spellcheck="false" />
        <span class="hint">Telegram's own, unless you run one.</span>
      </div>
      <p class="faint xs pair-note">
        People ask to talk to the agent with a pairing code you make here once the channel is on; you're asked before
        anyone is let in.
      </p>
    {/if}

    <div class="field">
      <label for="ch-mb">Largest file (MB)</label>
      <input
        id="ch-mb"
        class="input"
        style="width: 120px"
        type="number"
        min="1"
        max={type === "telegram" ? 20 : 1024}
        bind:value={uploadMb}
      />
      <span class="hint">
        Files people send are saved in the agent's folder, under uploads/.{type === "telegram"
          ? " Telegram gives bots files of up to 20 MB."
          : ""}
      </span>
    </div>
    <div class="row toggle-row">
      <div>
        <div class="row-title">Answer tool approvals in {kindLabel(type)}</div>
        <div class="row-sub">
          {#if type === "matrix"}
            Off: approvals wait for you here, in Chummy, and the room is told. On: /allow or /deny in the room answers
            them, so whoever has those accounts can let the agent use its tools.
          {:else}
            Off: approvals wait for you here, in Chummy, and the chat is told. On: Allow and Deny buttons in the chat
            answer them, so whoever you let in can let the agent use its tools.
          {/if}
        </div>
      </div>
      <Toggle checked={approvals} label={`Answer tool approvals in ${kindLabel(type)}`} onchange={(v) => (approvals = v)} />
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
        Add one to talk to an agent from Matrix or Telegram: messages start runs of its loop, and its replies come back
        as it works. Runs show in the sessions panel like any other.
        {#snippet actions()}
          <Button variant="primary" icon="plus" onclick={() => open("new")} disabled={!agents.length}>New channel</Button>
        {/snippet}
      </EmptyState>
    </div>
  {/if}

  {#each list as c (c.id)}
    {@const st = c.enabled ? c.live.state : "off"}
    {@const count = c.type === "matrix" ? c.users.length : c.people.length}
    <div class="card card-pad channel">
      <div class="c-head">
        <div class="c-main">
          <div class="c-name">
            {c.name}
            <span class="badge">{kindLabel(c.type)}</span>
            <span class="status" class:ok={st === "connected"} class:bad={st === "problem"}>{stateLabel[st] ?? st}</span>
          </div>
          <div class="muted small">
            <Icon name="bot" size={12} /> {agentName(c.agent)} · {count} {count === 1 ? "person" : "people"}
            {#if c.approvals}· approvals in {kindLabel(c.type)}{/if}
          </div>
          {#if c.type === "matrix"}
            <div class="faint xs mono">{c.user_id} on {c.homeserver}</div>
          {:else}
            <div class="faint xs">
              {#if c.live.account}<span class="mono">{c.live.account}</span>{" · "}{/if}not end-to-end encrypted{#if c.api_url !== TELEGRAM_API}{" · through "}<span
                  class="mono">{c.api_url}</span
                >{/if}
            </div>
          {/if}
          {#if c.enabled && c.live.message}<div class="xs problem">{c.live.message}</div>{/if}
        </div>
        <div class="c-actions">
          <Toggle checked={c.enabled} label="{c.name} on" onchange={(v) => toggle(c, v)} />
          <Button variant="ghost" icon="edit" iconOnly aria-label="Edit" onclick={() => open(c)} />
          <Button variant="ghost" icon="trash" iconOnly aria-label="Delete" onclick={() => remove(c)} />
        </div>
      </div>

      {#each c.live.pending as v (v.n)}
        <div class="verify">
          {#if v.kind === "emoji"}
            <div class="small"><b>{v.who}</b> wants to verify Chummy. Check that they see these emoji, in this order:</div>
            <div class="emoji">
              {#each v.emoji as [symbol, label], i (i)}
                <span class="em"><span class="sym">{symbol}</span><span class="lab">{label}</span></span>
              {/each}
            </div>
            <div class="v-actions">
              <Button variant="primary" onclick={() => answer(c, v.n, true)}>They match</Button>
              <Button variant="ghost" onclick={() => answer(c, v.n, false)}>They don't match</Button>
            </div>
          {:else}
            <div class="small">
              <b>{v.who}</b> sent the pairing code and wants to talk to {agentName(c.agent)}. Let them in only if it's the
              person you gave it to.
            </div>
            <div class="v-actions" style="margin-top: var(--space-3)">
              <Button variant="primary" onclick={() => answer(c, v.n, true)}>Let them in</Button>
              <Button variant="ghost" onclick={() => answer(c, v.n, false)}>Refuse</Button>
            </div>
          {/if}
        </div>
      {/each}

      {#if c.type === "telegram"}
        <div class="people">
          {#each c.people as p (p.id)}
            <div class="person">
              <Icon name="person" size={13} />
              <span class="small">{p.name}</span>
              <span class="faint xs mono">{p.id}</span>
              <span class="spacer"></span>
              <Button variant="ghost" onclick={() => removePerson(c, p.id)}>Remove</Button>
            </div>
          {/each}
          {#if c.live.code && c.live.code.expires_at > now}
            <div class="code">
              <div class="code-num mono">{c.live.code.code}</div>
              <div class="small">
                Ask them to send <span class="mono">/start {c.live.code.code}</span> to
                {#if c.live.account}<span class="mono">{c.live.account}</span>{:else}the bot{/if} in Telegram, within
                {minutesLeft(c.live.code.expires_at)}. You'll be asked here before they're let in. The code works once.
              </div>
            </div>
          {/if}
          <div class="p-actions">
            <Button
              variant={c.people.length ? "ghost" : "primary"}
              icon="plus"
              disabled={!c.enabled || st !== "connected"}
              onclick={() => makeCode(c)}
            >
              {c.live.code && c.live.code.expires_at > now ? "New code" : "Add a person"}
            </Button>
            {#if !c.enabled || st !== "connected"}<span class="faint xs">The channel has to be connected to add people.</span>{/if}
          </div>
        </div>
      {/if}
      {#if c.live.note}<div class="faint xs note">{c.live.note}</div>{/if}
    </div>
  {/each}

  {#if list.length}
    <p class="faint xs" style="margin-top: var(--space-4)">
      In a room or chat: send a message to talk to the agent, reply to its questions, and /help for the commands (/stop,
      /new, /status…). Chummy connects to the homeserver or Telegram itself, like model connections: the Internet
      permission doesn't apply.
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
  textarea.input {
    height: auto;
    padding: var(--space-2) var(--space-3);
    resize: vertical;
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
  .pair-note {
    margin: var(--space-2) 0 var(--space-4);
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
