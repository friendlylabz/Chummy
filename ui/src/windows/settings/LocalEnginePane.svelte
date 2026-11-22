<script lang="ts">
  import { commands, errorText } from "$lib/ipc";
  import { bytes, date } from "$lib/format";
  import { local } from "$lib/stores/local.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";
  import DownloadProgress from "./DownloadProgress.svelte";

  let { addModel }: { addModel: () => void } = $props();

  let token = $state("");
  let tokenError = $state("");
  let showInstallLog = $state(false);
  let engineLog = $state<string[] | null>(null);

  const s = $derived(local.status);
  const runtime = $derived(s?.runtime ?? null);
  const install = $derived(local.install);
  const engine = $derived(local.engine);
  const downloading = $derived(
    Object.entries(local.downloads).filter(([, d]) => !d.done && !d.error),
  );

  async function saveToken(value: string | null) {
    tokenError = "";
    try {
      await commands.setHfToken(value);
      token = "";
    } catch (e) {
      tokenError = errorText(e);
    }
  }

  async function showLog() {
    engineLog = engineLog ? null : await commands.engineLog();
  }
</script>

<h1>Local engine</h1>
<p class="pane-sub">
  Chummy's basic built-in way to run Hugging Face models on this computer. For speed, prefer Ollama, llama.cpp
  or vLLM in Models.
</p>

<div class="section-label">Runtime</div>
<div class="card card-pad">
  {#if install.running}
    <div class="row-title">Setting up the engine…</div>
    <p class="muted small">
      {install.step ? `Step ${install.step.index + 1} of ${install.step.count}: ${install.step.phase}` : "Starting…"}
    </p>
    <div style="margin: var(--space-3) 0"><ProgressBar value={install.step ? (install.step.index + 0.5) / install.step.count : null} label="Installing" /></div>
    {#if install.log.length}<div class="faint xs mono last">{install.log[install.log.length - 1]}</div>{/if}
    <div class="actions">
      <Button variant="ghost" onclick={() => (showInstallLog = !showInstallLog)}>{showInstallLog ? "Hide" : "Show"} details</Button>
      <span class="spacer"></span>
      <Button onclick={() => commands.cancelRuntimeInstall()}>Cancel</Button>
    </div>
    {#if showInstallLog}<pre class="log">{install.log.join("\n")}</pre>{/if}
  {:else if runtime}
    <div class="rt">
      <span class="badge ok">Installed</span>
      <span class="small">
        Running on <strong>{runtime.device === "cuda" ? runtime.gpu ?? "NVIDIA GPU" : runtime.device === "mps" ? "Apple GPU (Metal)" : "CPU"}</strong>
      </span>
    </div>
    <p class="faint xs mono" style="margin-top: var(--space-2)">
      Python {runtime.python} · PyTorch {runtime.torch}{runtime.cuda ? ` (CUDA ${runtime.cuda})` : ""} · transformers {runtime.transformers} · set up {date(runtime.installed_at)}
    </p>
    <div class="actions">
      <Button icon="refresh" onclick={() => local.startInstall()}>Repair</Button>
      <Button variant="ghost" icon="trash" onclick={async () => { if (await commands.removeRuntime()) local.refresh(); }}>Remove</Button>
    </div>
  {:else}
    <div class="row-title">Not set up yet</div>
    <p class="muted small">
      Needs Python, PyTorch and transformers (about {s?.runtime_download_gb.toFixed(1) ?? "1–4"} GB). Chummy
      installs them into its own folder the first time you run a local model, or now if you prefer.
    </p>
    {#if install.error}<div class="notice danger" style="margin-top: var(--space-3)"><Icon name="x-circle" size={16} /><span class="pre-wrap">{install.error}</span></div>{/if}
    <div class="actions">
      <Button variant="primary" icon="download" onclick={() => local.startInstall()}>Set up now</Button>
      {#if install.log.length}<Button variant="ghost" onclick={() => (showInstallLog = !showInstallLog)}>Details</Button>{/if}
    </div>
    {#if showInstallLog && install.log.length}<pre class="log">{install.log.join("\n")}</pre>{/if}
  {/if}
  {#if s}<p class="faint xs" style="margin-top: var(--space-3)">This computer: {s.hardware_summary}</p>{/if}
</div>

<div class="card rows">
  <div class="row">
    <div>
      <div class="row-title">Engine</div>
      <div class="row-sub">
        {#if engine.state === "ready"}Running {engine.model} on {engine.device}.
        {:else if engine.state === "loading"}Loading {engine.model}…
        {:else if engine.state === "failed"}<span class="pre-wrap">Failed: {engine.error}</span>
        {:else}Stopped. It starts by itself when a local model is used.{/if}
      </div>
    </div>
    <div class="row-actions">
      <Button variant="ghost" onclick={showLog}>{engineLog ? "Hide log" : "Log"}</Button>
      <Button icon="stop" onclick={() => commands.stopEngine()} disabled={engine.state === "stopped"}>Stop</Button>
    </div>
  </div>
  {#if engineLog}<div style="padding: 0 var(--space-4) var(--space-4)"><pre class="log">{engineLog.join("\n") || "(empty)"}</pre></div>{/if}
  <div class="row">
    <div>
      <div class="row-title">Device</div>
      <div class="row-sub">Where models run. Auto picks an NVIDIA or Apple GPU when there is one.</div>
    </div>
    <select class="input" style="width: 180px" value={settings.current.local.device} onchange={(e) => commands.setLocalDevice((e.currentTarget as HTMLSelectElement).value)}>
      <option value="auto">Auto</option>
      <option value="cuda">NVIDIA GPU (CUDA)</option>
      <option value="mps">Apple GPU (Metal)</option>
      <option value="cpu">CPU only</option>
    </select>
  </div>
</div>

<div class="section-label">Downloaded models</div>
<div class="card">
  {#if downloading.length}
    {#each downloading as [repo, d] (repo)}
      <div class="model"><div class="mono small" style="margin-bottom: var(--space-2)">{repo}</div><DownloadProgress {repo} progress={d.progress} /></div>
    {/each}
  {/if}
  {#each s?.models ?? [] as m (m.dir_name)}
    <div class="model m-row">
      <div style="flex: 1; min-width: 0">
        <div class="mono small">{m.manifest.repo}</div>
        <div class="faint xs">{bytes(m.manifest.bytes)} · tool format {m.manifest.parser} · {date(m.manifest.downloaded_at)}</div>
      </div>
      <Button variant="ghost" icon="trash" iconOnly aria-label="Delete" onclick={async () => { if (await commands.deleteLocalModel(m.dir_name)) local.refresh(); }} />
    </div>
  {/each}
  {#if !downloading.length && !s?.models.length}
    <div class="model muted small">None yet.</div>
  {/if}
  <div class="model"><Button icon="plus" onclick={addModel}>Add a model</Button></div>
</div>

<div class="section-label">Hugging Face token</div>
<div class="card card-pad">
  <p class="muted small" style="margin-bottom: var(--space-3)">
    Only needed for gated models such as Llama, after you accept their license on huggingface.co. Stored in your
    system keychain.
  </p>
  <div class="input-row">
    <input class="input mono" type="password" autocomplete="off" bind:value={token} placeholder={settings.current.hf_token_set ? "Saved. Paste a new one to replace it." : "hf_…"} />
    <Button onclick={() => saveToken(token)} disabled={!token.trim()}>Save</Button>
    {#if settings.current.hf_token_set}<Button variant="ghost" onclick={() => saveToken(null)}>Remove</Button>{/if}
  </div>
  {#if tokenError}<p class="small" style="color: var(--danger); margin-top: var(--space-2)">{tokenError}</p>{/if}
</div>

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .rt {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }
  .last {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row-actions {
    display: flex;
    gap: 2px;
  }
  .model {
    padding: var(--space-3) var(--space-4);
  }
  .model + .model {
    border-top: 1px solid var(--border);
  }
  .m-row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
</style>
