<script lang="ts">
  import { onMount } from "svelte";
  import {
    commands,
    errorText,
    type ChummydDraft,
    type ChummydMatrix,
    type ChummydReport,
    type ChummydSources,
  } from "$lib/ipc";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import Toggle from "$lib/components/Toggle.svelte";

  let sources = $state<ChummydSources>({ agents: [], jobs: [], time_zone: "UTC" });
  let draft = $state<ChummydDraft>({
    name: "",
    time_zone: "UTC",
    max_runs: 1,
    chat_messages: 2000,
    chats: 50,
    agents: [],
    approvals: [],
    jobs: [],
    terminal: true,
    message_text: false,
    base: null,
  });
  /** The bundle being changed, if one was opened. */
  let baseName = $state("");
  let missing = $state<string[]>([]);
  let report = $state<ChummydReport>({ errors: [], warnings: [] });
  let password = $state("");
  let password2 = $state("");
  let passwordProblem = $state<string | null>(null);
  let generated = $state("");
  let error = $state("");
  let saved = $state("");
  let busy = $state(false);
  let opening = $state(false);
  let openPassword = $state("");
  /** What checking the homeserver said. */
  let hsResult = $state<{ ok: boolean; text: string } | null>(null);
  let hsBusy = $state(false);

  function newMatrix(): ChummydMatrix {
    return {
      homeserver: "https://",
      user_id: "",
      password: "",
      has_password: false,
      users: {},
      report_to: {},
      upload_mb: 100,
    };
  }

  function setMatrix(on: boolean) {
    draft.matrix = on ? (draft.matrix ?? newMatrix()) : null;
    hsResult = null;
  }

  /** Matrix IDs typed as a list (commas, spaces or new lines between). */
  function setUsers(agent: string, text: string) {
    if (!draft.matrix) return;
    const ids = text
      .split(/[\s,]+/)
      .map((x) => x.trim())
      .filter(Boolean);
    draft.matrix.users = { ...draft.matrix.users, [agent]: ids };
  }

  function setReportTo(job: string, user: string) {
    if (!draft.matrix) return;
    const r = { ...draft.matrix.report_to };
    if (user) r[job] = user;
    else delete r[job];
    draft.matrix.report_to = r;
  }

  async function checkHomeserver() {
    if (!draft.matrix) return;
    hsBusy = true;
    hsResult = null;
    try {
      hsResult = { ok: true, text: await commands.chummydCheckHomeserver(draft.matrix.homeserver) };
    } catch (e) {
      hsResult = { ok: false, text: errorText(e) };
    } finally {
      hsBusy = false;
    }
  }

  onMount(async () => {
    sources = await commands.chummydSources();
    draft.time_zone = sources.time_zone;
    draft.name = "My chummyd";
  });

  // Checked as it changes, with the same checks chummyd runs at start-up.
  $effect(() => {
    const d = $state.snapshot(draft);
    const t = setTimeout(async () => {
      report = await commands.chummydCheck(d);
    }, 250);
    return () => clearTimeout(t);
  });

  $effect(() => {
    const p = password;
    if (!p) {
      passwordProblem = null;
      return;
    }
    const t = setTimeout(async () => {
      passwordProblem = await commands.chummydPasswordProblem(p);
    }, 250);
    return () => clearTimeout(t);
  });

  const agentName = (id: string) => sources.agents.find((a) => a.id === id)?.name ?? id;

  function toggle(list: string[], id: string, on: boolean): string[] {
    return on ? [...new Set([...list, id])] : list.filter((x) => x !== id);
  }

  function includeAgent(id: string, on: boolean) {
    draft.agents = toggle(draft.agents, id, on);
    if (!on) {
      draft.approvals = toggle(draft.approvals, id, false);
      draft.jobs = draft.jobs.filter((j) => sources.jobs.find((x) => x.id === j)?.agent !== id);
    }
  }

  async function generate() {
    error = "";
    try {
      generated = await commands.chummydGeneratePassword();
      password = password2 = generated;
    } catch (e) {
      error = errorText(e);
    }
  }

  async function copyGenerated() {
    try {
      await navigator.clipboard.writeText(generated);
    } catch {
      /* Some systems don't allow it; it can still be selected. */
    }
  }

  async function openBundle() {
    error = "";
    busy = true;
    try {
      const opened = await commands.chummydOpenBundle(openPassword);
      if (!opened) return;
      draft = { ...opened.draft };
      baseName = opened.draft.name;
      missing = opened.missing;
      opening = false;
      saved = "";
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
      openPassword = "";
    }
  }

  /** Puts the cursor in a field as it appears. */
  function focusOnShow(node: HTMLElement) {
    node.focus();
  }

  const ready = $derived(
    report.errors.length === 0 && !!password && !passwordProblem && password === password2 && draft.agents.length > 0,
  );

  async function exportBundle() {
    error = "";
    saved = "";
    busy = true;
    try {
      const path = await commands.chummydExport($state.snapshot(draft), password);
      if (path) {
        saved = path;
        password = password2 = generated = "";
      }
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<main class="page">
  <div class="inner">
    <h1>Configure chummyd</h1>
    <p class="pane-sub">
      chummyd runs your agents unattended, on this computer or another one: their jobs, and conversations with them.
      Choose what it runs, then export a bundle, one encrypted file with everything it needs. chummyd runs exactly
      that until it stops; to change it, export again and start chummyd again.
    </p>

    <div class="actions top">
      <Button icon="import" onclick={() => ((opening = true), (error = ""))}>Open a bundle…</Button>
      <span class="muted small">To change a bundle chummyd already uses, open it first, so its conversations and paused runs carry over.</span>
    </div>

    {#if baseName}
      <div class="notice"><Icon name="info" size={16} /><span>Changing “{baseName}”. Exporting replaces it, as the same chummyd.</span></div>
      {#each missing as m (m)}<div class="notice warn"><Icon name="alert" size={16} /><span>{m} It's left out.</span></div>{/each}
    {/if}

    <div class="section-label">chummyd</div>
    <div class="card rows">
      <div class="row">
        <div><div class="row-title">Name</div><div class="row-sub">To tell your chummyds apart.</div></div>
        <input class="input narrow" bind:value={draft.name} maxlength="80" aria-label="Name" />
      </div>
      <div class="row">
        <div><div class="row-title">Time zone</div><div class="row-sub">For job schedules, and the time tools see.</div></div>
        <input class="input narrow mono" bind:value={draft.time_zone} aria-label="Time zone" />
      </div>
      <div class="row">
        <div><div class="row-title">Job runs at once</div><div class="row-sub">Conversations aren't limited by this.</div></div>
        <select class="input narrow" bind:value={draft.max_runs} aria-label="Job runs at once">
          {#each [1, 2, 3, 4, 5, 6, 7, 8] as n (n)}<option value={n}>{n}</option>{/each}
        </select>
      </div>
      <div class="row">
        <div>
          <div class="row-title">Messages kept per chat</div>
          <div class="row-sub">Older messages are dropped. 10 to 100,000.</div>
        </div>
        <input class="input narrow" type="number" min="10" max="100000" step="100" bind:value={draft.chat_messages} aria-label="Messages kept per chat" />
      </div>
      <div class="row">
        <div>
          <div class="row-title">Chats kept per agent</div>
          <div class="row-sub">When there are more, the least recently used is deleted. 1 to 10,000.</div>
        </div>
        <input class="input narrow" type="number" min="1" max="10000" bind:value={draft.chats} aria-label="Chats kept per agent" />
      </div>
    </div>

    <div class="section-label">Agents</div>
    {#if sources.agents.length === 0}
      <div class="card card-pad muted small">There are no agents yet. Make one in Settings → Agents.</div>
    {:else}
      <div class="card rows">
        {#each sources.agents as a (a.id)}
          {@const on = draft.agents.includes(a.id)}
          <div class="row">
            <div>
              <div class="row-title">{a.name}</div>
              <div class="row-sub">
                {a.loop_name} · {a.model ?? "no model"}{#if a.tools.length} · tools: {a.tools.join(", ")}{a.network ? ", online" : ""}{/if}
              </div>
              {#if a.builtin_engine}<div class="row-sub warn-text">Its model is Chummy's built-in engine, which chummyd doesn't have. Give it another model first.</div>{/if}
              <div class="row-sub">Its folder in chummyd: <span class="mono">&lt;jail dir&gt;/{a.id}/</span>, not its folder here.</div>
            </div>
            <Toggle checked={on} label={`Include ${a.name}`} onchange={(v) => includeAgent(a.id, v)} />
          </div>
          {#if on}
            <div class="row sub-row">
              <div>
                <div class="row-title">Answer its approvals over channels</div>
                <div class="row-sub">
                  Tool approvals and daily-limit questions can be answered from a channel. Off: they're refused there,
                  so tools that ask first won't run. A remote “allow” is what a prompt injection would want: turn on
                  only if you need it.
                </div>
              </div>
              <Toggle
                checked={draft.approvals.includes(a.id)}
                label={`Approvals over channels for ${a.name}`}
                onchange={(v) => (draft.approvals = toggle(draft.approvals, a.id, v))}
              />
            </div>
          {/if}
        {/each}
      </div>
    {/if}

    <div class="section-label">Jobs</div>
    {#if sources.jobs.length === 0}
      <div class="card card-pad muted small">There are no jobs. Make them in the Automation menu.</div>
    {:else}
      <div class="card rows">
        {#each sources.jobs as j (j.id)}
          {@const agentOn = draft.agents.includes(j.agent)}
          <div class="row">
            <div>
              <div class="row-title">{j.name}</div>
              <div class="row-sub">Runs as {agentName(j.agent)}{agentOn ? "" : ": include that agent first"}</div>
            </div>
            <Toggle
              checked={draft.jobs.includes(j.id)}
              disabled={!agentOn}
              label={`Include ${j.name}`}
              onchange={(v) => (draft.jobs = toggle(draft.jobs, j.id, v))}
            />
          </div>
        {/each}
      </div>
    {/if}

    <div class="section-label">Channels</div>
    <div class="card rows">
      <div class="row">
        <div>
          <div class="row-title">Terminal</div>
          <div class="row-sub">Talk to the agents in the terminal chummyd runs in. Whoever has that terminal has already entered the password.</div>
        </div>
        <Toggle checked={draft.terminal} label="Terminal channel" onchange={(v) => (draft.terminal = v)} />
      </div>
      <div class="row">
        <div>
          <div class="row-title">Matrix</div>
          <div class="row-sub">
            chummyd logs in as its own Matrix account, makes an end-to-end encrypted room for each agent and each
            person allowed to talk to it, and invites them. People verify chummyd by emoji from their Matrix app; you
            confirm the emoji in chummyd's terminal with <span class="mono">/verify</span>.
          </div>
        </div>
        <Toggle checked={!!draft.matrix} label="Matrix channel" onchange={setMatrix} />
      </div>
      {#if draft.matrix}
        {@const m = draft.matrix}
        <div class="row sub-row">
          <div><div class="row-title">Homeserver</div><div class="row-sub">Its address, like https://matrix.example.org.</div></div>
          <div class="stack">
            <input class="input narrow mono" bind:value={m.homeserver} aria-label="Homeserver" spellcheck="false" />
            <Button variant="ghost" disabled={hsBusy || !m.homeserver} onclick={checkHomeserver}>Check the homeserver</Button>
          </div>
        </div>
        {#if hsResult}
          <div class="row sub-row">
            <span class="small" class:good={hsResult.ok} class:warn-text={!hsResult.ok}>{hsResult.text}</span>
          </div>
        {/if}
        <div class="row sub-row">
          <div><div class="row-title">chummyd's Matrix ID</div><div class="row-sub">An account just for chummyd, like @chummyd:example.org.</div></div>
          <input class="input narrow mono" bind:value={m.user_id} aria-label="chummyd's Matrix ID" spellcheck="false" placeholder="@chummyd:example.org" />
        </div>
        <div class="row sub-row">
          <div>
            <div class="row-title">Its password</div>
            <div class="row-sub">To log in the first time and set up its cross-signing. It's kept in the bundle, encrypted.</div>
          </div>
          <input
            class="input narrow"
            type="password"
            autocomplete="off"
            bind:value={m.password}
            aria-label="chummyd's Matrix password"
            placeholder={m.has_password ? "Kept from the bundle" : ""}
          />
        </div>
        <div class="row sub-row">
          <div><div class="row-title">Largest file</div><div class="row-sub">Files people send go to the agent's <span class="mono">uploads/</span> folder.</div></div>
          <div class="stack">
            <input class="input tiny" type="number" min="1" max="1024" bind:value={m.upload_mb} aria-label="Largest file, in MB" />
            <span class="small muted">MB</span>
          </div>
        </div>
        {#each draft.agents as id (id)}
          <div class="row sub-row">
            <div>
              <div class="row-title">Who may talk to {agentName(id)}</div>
              <div class="row-sub">Matrix IDs, separated by commas. Each gets a room of their own.</div>
            </div>
            <input
              class="input narrow mono"
              value={(m.users[id] ?? []).join(", ")}
              onchange={(e) => setUsers(id, e.currentTarget.value)}
              aria-label={`Who may talk to ${agentName(id)}`}
              spellcheck="false"
              placeholder="@you:example.org"
            />
          </div>
        {/each}
        {#each draft.jobs as jid (jid)}
          {@const j = sources.jobs.find((x) => x.id === jid)}
          {#if j}
            <div class="row sub-row">
              <div>
                <div class="row-title">“{j.name}” reports to</div>
                <div class="row-sub">Its results and questions go to this person's room with {agentName(j.agent)}.</div>
              </div>
              <select
                class="input narrow"
                value={m.report_to[jid] ?? ""}
                onchange={(e) => setReportTo(jid, e.currentTarget.value)}
                aria-label={`Who “${j.name}” reports to`}
              >
                <option value="">Nobody</option>
                {#each m.users[j.agent] ?? [] as u (u)}<option value={u}>{u}</option>{/each}
              </select>
            </div>
          {/if}
        {/each}
      {/if}
    </div>

    <div class="section-label">Logs</div>
    <div class="card rows">
      <div class="row">
        <div>
          <div class="row-title">Include message text</div>
          <div class="row-sub">Off: chummyd's logs say only what happened (runs, statuses, counts), never what was said.</div>
        </div>
        <Toggle checked={draft.message_text} label="Include message text in logs" onchange={(v) => (draft.message_text = v)} />
      </div>
    </div>

    <div class="section-label">Check</div>
    <div class="card card-pad">
      {#each report.errors as e (e)}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{e}</span></div>{/each}
      {#each report.warnings as w (w)}<div class="notice warn"><Icon name="alert" size={16} /><span>{w}</span></div>{/each}
      {#if report.errors.length === 0}
        <div class="ok"><Icon name="check-circle" size={16} /><span>chummyd would start with this.</span></div>
      {/if}
    </div>

    <div class="section-label">Password</div>
    <div class="card card-pad">
      <p class="muted small explain">
        The bundle holds the API keys of the model connections it uses. Anyone who has both the bundle and its
        password can read them, so use a strong password and keep the two safe, and apart. If it's lost, export a new
        bundle: nothing can open this one without it.
      </p>
      <div class="field">
        <label for="pw">Password</label>
        <input id="pw" class="input" type="password" autocomplete="new-password" bind:value={password} />
      </div>
      <div class="field">
        <label for="pw2">Password again</label>
        <input id="pw2" class="input" type="password" autocomplete="new-password" bind:value={password2} />
      </div>
      {#if passwordProblem}<p class="small warn-text">{passwordProblem}</p>{/if}
      {#if password && password2 && password !== password2}<p class="small warn-text">The passwords don't match.</p>{/if}
      <div class="actions">
        <Button icon="key" onclick={generate}>Generate a password</Button>
        {#if generated}
          <span class="mono small" data-selectable>{generated}</span>
          <Button variant="ghost" onclick={copyGenerated}>Copy</Button>
        {/if}
      </div>
    </div>

    {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
    {#if saved}
      <div class="notice">
        <Icon name="check-circle" size={16} />
        <span>
          Saved to <span class="mono" data-selectable>{saved}</span>. Start it with
          <span class="mono" data-selectable>chummyd run --bundle {saved} --jail-dir &lt;folder&gt; --state &lt;folder&gt;</span>
        </span>
      </div>
    {/if}
    <div class="actions bottom">
      <span class="spacer"></span>
      <Button variant="primary" icon="export" disabled={!ready || busy} onclick={exportBundle}>Export bundle…</Button>
    </div>
  </div>
</main>

{#if opening}
  <Modal title="Open a bundle" onclose={() => (opening = false)}>
    <p class="muted small">Enter the bundle's password, then pick the file.</p>
    <div class="field">
      <label for="open-pw">Password</label>
      <input id="open-pw" class="input" type="password" autocomplete="current-password" bind:value={openPassword} use:focusOnShow />
    </div>
    {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
    {#snippet actions()}
      <Button onclick={() => (opening = false)}>Cancel</Button>
      <Button variant="primary" disabled={!openPassword || busy} onclick={openBundle}>Choose the bundle…</Button>
    {/snippet}
  </Modal>
{/if}

<style>
  .page {
    height: 100%;
    overflow-y: auto;
  }
  .inner {
    max-width: 760px;
    margin: 0 auto;
    padding: var(--space-6);
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .top {
    margin-bottom: var(--space-4);
    align-items: center;
  }
  .bottom {
    margin-top: var(--space-5);
  }
  .sub-row {
    padding-left: var(--space-6);
    background: var(--surface-2, transparent);
  }
  .ok {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--ok, var(--accent));
    font-size: var(--text-sm);
  }
  .warn-text {
    color: var(--warn, #b26b00);
  }
  .stack {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .tiny {
    width: 6em;
  }
  .good {
    color: var(--ok, var(--accent));
  }
  .explain {
    margin-bottom: var(--space-3);
  }
  .notice {
    margin-bottom: var(--space-2);
  }
</style>
