<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, type LogKeyStatus, type Logging, type RunLogLevel, type SystemLogLevel } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import KeyTip from "$lib/components/KeyTip.svelte";

  /** Open the export or import dialog straight away. */
  let { start = null }: { start?: "export" | "import" | null } = $props();

  const logging = $derived(settings.current.logging);
  let status = $state<LogKeyStatus>({ fingerprint: null, error: null });
  let dialog = $state<"export" | "import" | null>(null);
  let pass = $state("");
  let pass2 = $state("");
  let busy = $state(false);
  let error = $state("");
  let notice = $state("");

  const runLevels: { value: RunLogLevel; label: string; hint: string }[] = [
    { value: "verbose", label: "Everything", hint: "Every block's inputs and outputs, and every model call, in full." },
    { value: "standard", label: "Shortened", hint: `The same, with each value cut to 500 characters.` },
    { value: "summary", label: "Steps only", hint: "Which blocks ran, model and tool calls, questions and replies, without the text in between." },
    { value: "off", label: "Off", hint: "No run logs. Chat's “What happened” and the Logs window have nothing to show." },
  ];
  const systemLevels: { value: SystemLogLevel; label: string }[] = [
    { value: "debug", label: "Everything, with each model call" },
    { value: "info", label: "What Chummy does (recommended)" },
    { value: "warnings", label: "Warnings and errors" },
    { value: "errors", label: "Errors only" },
    { value: "off", label: "Off" },
  ];

  async function refresh() {
    status = await commands.logKeyStatus();
  }
  onMount(() => {
    refresh();
    if (start) open(start);
  });

  async function set(patch: Partial<Logging>) {
    error = "";
    try {
      await commands.setLogging({ ...logging, ...patch });
      refresh();
    } catch (e) {
      error = errorText(e);
    }
  }

  function open(which: "export" | "import") {
    dialog = which;
    pass = pass2 = "";
    error = "";
    notice = "";
  }

  const exportBad = $derived(pass.length < 10 || pass !== pass2);

  async function run() {
    busy = true;
    error = "";
    try {
      if (dialog === "export") {
        if (await commands.exportLogKey(pass)) notice = "The log key was exported. Keep the file and its passphrase safe, and apart.";
        else return;
      } else {
        const fp = await commands.importLogKey(pass);
        if (!fp) return;
        notice = `Imported. This computer's log key is now ${fp}.`;
      }
      dialog = null;
      refresh();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
      pass = pass2 = "";
    }
  }
</script>

<h1>Logs</h1>
<p class="pane-sub">What Chummy writes down about runs and about itself. Logs stay on this computer and are deleted after 30 days.</p>

<div class="section-label">Run logs</div>
<div class="card rows">
  <div class="row">
    <div>
      <div class="row-title">Detail</div>
      <div class="row-sub">{runLevels.find((l) => l.value === logging.runs)?.hint}</div>
    </div>
    <select class="input narrow" value={logging.runs} onchange={(e) => set({ runs: e.currentTarget.value as RunLogLevel })} aria-label="Run log detail">
      {#each runLevels as l (l.value)}<option value={l.value}>{l.label}</option>{/each}
    </select>
  </div>
  <div class="row">
    <div>
      <div class="row-title">Encrypt run logs <KeyTip /></div>
      <div class="row-sub">
        Run logs can hold your messages, answers, text from files and anything secret that passed through a run. With
        this on, they're sealed with a key kept in your system keychain (XChaCha20-Poly1305).
      </div>
    </div>
    <Toggle checked={logging.encrypt} label="Encrypt run logs" disabled={logging.runs === "off"} onchange={(v) => set({ encrypt: v })} />
  </div>
</div>

<div class="section-label">Log key</div>
<div class="card card-pad">
  {#if status.error}
    <div class="notice danger"><Icon name="x-circle" size={16} /><span>{status.error} Encrypted run logs can't be written or read until the keychain works.</span></div>
  {:else if status.fingerprint}
    <div class="key">
      <Icon name="key" size={18} />
      <div>
        <div class="row-title">This computer's log key</div>
        <div class="mono small" data-selectable>{status.fingerprint}</div>
      </div>
    </div>
  {:else}
    <p class="muted small">There's no log key yet. One is made the first time an encrypted log is written.</p>
  {/if}
  <p class="muted small explain">
    Export the key to keep a backup, or to read these logs on another computer; import it there. The file is locked
    with a passphrase you choose. Without the key, encrypted logs can't be read by anyone, including you.
  </p>
  <div class="actions">
    <Button icon="export" onclick={() => open("export")}>Export key…</Button>
    <Button icon="import" onclick={() => open("import")}>Import key…</Button>
    <span class="spacer"></span>
    <Button variant="ghost" icon="history" onclick={() => commands.openWindow("logs")}>Open Logs</Button>
  </div>
</div>

<div class="section-label">System log</div>
<div class="card rows">
  <div class="row">
    <div>
      <div class="row-title">Detail</div>
      <div class="row-sub">What Chummy does: starts, settings and permission changes, runs, jobs, tools, questions and errors. Never your messages, files or keys, so it isn't encrypted.</div>
    </div>
    <select class="input narrow" value={logging.system} onchange={(e) => set({ system: e.currentTarget.value as SystemLogLevel })} aria-label="System log detail">
      {#each systemLevels as l (l.value)}<option value={l.value}>{l.label}</option>{/each}
    </select>
  </div>
</div>
{#if error && !dialog}<div class="notice danger" style="margin-top: var(--space-3)"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
{#if notice}<div class="notice" style="margin-top: var(--space-3)"><Icon name="check-circle" size={16} /><span>{notice}</span></div>{/if}

{#if dialog}
  <Modal title={dialog === "export" ? "Export the log key" : "Import a log key"} onclose={() => (dialog = null)}>
    {#if dialog === "export"}
      <p class="muted small">
        Choose a passphrase to lock the file (at least 10 characters). Anyone with both the file and the passphrase can
        read your encrypted logs, so keep them apart.
      </p>
    {:else}
      <p class="muted small">
        Pick a key file next, and enter the passphrase it was exported with. It replaces this computer's key: logs
        written with the current key stay unreadable here unless you export it first.
      </p>
    {/if}
    <div class="field">
      <label for="lk-pass">Passphrase</label>
      <input id="lk-pass" class="input" type="password" autocomplete="new-password" bind:value={pass} />
    </div>
    {#if dialog === "export"}
      <div class="field">
        <label for="lk-pass2">Passphrase again</label>
        <input id="lk-pass2" class="input" type="password" autocomplete="new-password" bind:value={pass2} />
        {#if pass2 && pass !== pass2}<span class="hint">The passphrases don't match.</span>{/if}
      </div>
    {/if}
    {#if error}<div class="notice danger"><Icon name="x-circle" size={16} /><span>{error}</span></div>{/if}
    {#snippet actions()}
      <Button variant="ghost" onclick={() => (dialog = null)}>Cancel</Button>
      <Button variant="primary" disabled={busy || (dialog === "export" ? exportBad : !pass)} onclick={run}>
        {busy ? "Working…" : dialog === "export" ? "Choose where to save…" : "Choose the key file…"}
      </Button>
    {/snippet}
  </Modal>
{/if}

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .narrow {
    width: 230px;
    flex: none;
  }
  .row-title {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .key {
    display: flex;
    gap: var(--space-3);
    align-items: center;
    color: var(--accent);
  }
  .key > div {
    color: var(--text);
  }
  .explain {
    margin: var(--space-3) 0;
  }
  .spacer {
    flex: 1;
  }
</style>
