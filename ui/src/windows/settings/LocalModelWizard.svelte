<script lang="ts">
  import { commands, errorText, type Inspection, type Precision } from "$lib/ipc";
  import { bytes, context, params, precisionLabel } from "$lib/format";
  import { local } from "$lib/stores/local.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import StatusIcon from "$lib/components/StatusIcon.svelte";
  import DownloadProgress from "./DownloadProgress.svelte";

  let { oncancel, ondone }: { oncancel: () => void; ondone: (providerId: string) => void } = $props();

  const suggestions = ["Qwen/Qwen2.5-1.5B-Instruct", "Qwen/Qwen3-4B-Instruct-2507", "ibm-granite/granite-3.3-2b-instruct"];

  let repo = $state("");
  let checking = $state(false);
  let error = $state("");
  let result = $state<Inspection | null>(null);
  let precision = $state<Precision>("half");
  let showTable = $state(false);
  let started = $state(false);

  const hasNvidia = $derived((result?.hardware.nvidia.length ?? 0) > 0);
  const dl = $derived(result ? local.downloads[result.repo] : undefined);
  const thisComputer = $derived(result?.estimates?.fits.find((f) => f.this_computer));

  local.onDownloaded = (r, id) => {
    if (result && r === result.repo) ondone(id);
  };

  async function inspect() {
    if (!repo.trim()) return;
    checking = true;
    error = "";
    result = null;
    started = false;
    try {
      result = await commands.inspectModel(repo.trim());
      precision = result.recommended_precision;
    } catch (e) {
      error = errorText(e);
    } finally {
      checking = false;
    }
  }

  async function download() {
    if (!result) return;
    error = "";
    try {
      await commands.downloadModel(result.repo, precision);
      started = true;
      local.downloads[result.repo] = { progress: null, error: null, done: false };
    } catch (e) {
      error = errorText(e);
    }
  }

  function fitText(best: { precision: string; context: number; bytes: number } | null) {
    if (!best) return "Won't fit";
    return `${precisionLabel[best.precision]}, up to ${context(best.context)} context · ~${bytes(best.bytes)}`;
  }
</script>

<button class="back" type="button" onclick={oncancel}><Icon name="chevron-left" size={16} />Models</button>
<h1>Add a model to the built-in engine</h1>
<p class="pane-sub">
  Downloads a model from Hugging Face and runs it on this computer with PyTorch and transformers.
</p>
<div class="notice basic">
  <Icon name="info" size={16} />
  <span>
    <strong>A basic engine.</strong> It runs one request at a time and is much slower and less capable than
    vLLM, llama.cpp or Ollama. Use those (under Models) when you can; this is for running a model straight
    from Hugging Face with nothing else installed.
  </span>
</div>

<div class="card card-pad">
  <div class="field">
    <label for="lw-repo">Hugging Face model</label>
    <div class="input-row">
      <input
        id="lw-repo"
        class="input mono"
        bind:value={repo}
        placeholder="owner/model, or a huggingface.co link"
        spellcheck="false"
        onkeydown={(e) => e.key === "Enter" && inspect()}
      />
      <Button variant="primary" icon="search" onclick={inspect} disabled={checking || !repo.trim()}>
        {checking ? "Checking…" : "Check model"}
      </Button>
    </div>
    <span class="hint">
      Try:
      {#each suggestions as s, i (s)}
        <button class="link" type="button" onclick={() => ((repo = s), inspect())}>{s}</button>{i < suggestions.length - 1 ? " · " : ""}
      {/each}
    </span>
  </div>
  {#if error}
    <div class="notice danger" style="margin-top: var(--space-4)"><Icon name="x-circle" size={16} /><span class="pre-wrap">{error}</span></div>
  {/if}
</div>

{#if result}
  <div class="card card-pad">
    <div class="head">
      <div>
        <div class="repo mono">{result.repo}</div>
        <div class="faint xs mono">{result.architecture ?? "unknown architecture"} · {result.revision.slice(0, 10)}</div>
      </div>
      {#if result.ok}<span class="badge ok">Can be used</span>{:else}<span class="badge danger">Can't be used</span>{/if}
    </div>

    <div class="facts">
      <div><span class="k">Parameters</span><span class="v">{params(result.params)}</span></div>
      <div><span class="k">Download</span><span class="v">{bytes(result.download_bytes)}</span></div>
      <div><span class="k">Max context</span><span class="v">{result.context_length ? context(result.context_length) : "—"}</span></div>
      <div><span class="k">Tool format</span><span class="v">{result.parser ?? "—"}</span></div>
    </div>

    <ul class="checks">
      {#each result.checks as c (c.label)}
        <li><StatusIcon status={c.status} /><div><span class="cl">{c.label}</span> <span class="muted small">{c.detail}</span></div></li>
      {/each}
    </ul>
  </div>

  {#if result.estimates}
    <div class="card card-pad">
      <div class="row-title">Memory needed</div>
      <p class="muted small" style="margin-bottom: var(--space-3)">
        Rough estimates: weights + conversation memory (KV cache) + overhead. Longer chats need more.
      </p>
      <table class="fits">
        <tbody>
          {#each result.estimates.fits as f (f.label)}
            <tr class:me={f.this_computer}>
              <td>{f.this_computer ? "This computer" : f.label}{#if f.this_computer}<div class="faint xs">{f.label}</div>{/if}</td>
              <td class:no={!f.best}>{fitText(f.best)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <button class="link" type="button" onclick={() => (showTable = !showTable)} style="margin-top: var(--space-3)">
        {showTable ? "Hide" : "Show"} all sizes
      </button>
      {#if showTable}
        <table class="grid">
          <thead><tr><th></th>{#each result.estimates.table.filter((c) => c.precision === "half") as c (c.context)}<th>{context(c.context)} context</th>{/each}</tr></thead>
          <tbody>
            {#each ["half", "int8", "int4"] as p (p)}
              <tr>
                <th>{precisionLabel[p]}</th>
                {#each result.estimates.table.filter((c) => c.precision === p) as c (c.context)}<td>{bytes(c.bytes)}</td>{/each}
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/if}

  {#if result.ok}
    <div class="card card-pad">
      <div class="field">
        <span class="label">Precision</span>
        <div class="seg" role="radiogroup" aria-label="Precision">
          {#each ["half", "int8", "int4"] as p (p)}
            <button
              type="button"
              role="radio"
              aria-checked={precision === p}
              disabled={p !== "half" && !hasNvidia}
              onclick={() => (precision = p as Precision)}
            >{precisionLabel[p]}{p === result.recommended_precision ? " · recommended" : ""}</button>
          {/each}
        </div>
        <span class="hint">
          {#if hasNvidia}8-bit and 4-bit use less memory with some loss of quality.{:else}8-bit and 4-bit need an NVIDIA GPU.{/if}
          {#if thisComputer && !thisComputer.best} This computer probably doesn't have enough memory for this model.{/if}
        </span>
      </div>

      <div class="actions">
        {#if dl && !dl.done && !dl.error}
          <div style="flex: 1"><DownloadProgress repo={result.repo} progress={dl.progress} /></div>
        {:else}
          {#if dl?.error}<div class="notice danger" style="flex: 1"><Icon name="x-circle" size={16} /><span>{dl.error}</span></div>{/if}
          <span class="spacer"></span>
          <Button variant="primary" icon="download" onclick={download}>
            {dl?.error || started ? "Try again" : `Download ${bytes(result.download_bytes)}`}
          </Button>
        {/if}
      </div>
      <p class="faint xs" style="margin-top: var(--space-3)">
        Saved in Chummy's own folder. Each file's checksum is verified. Interrupted downloads resume.
      </p>
    </div>
  {/if}
{/if}

<style>
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
  }
  .basic {
    margin-bottom: var(--space-4);
  }
  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: inherit;
    font-family: var(--font-mono);
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-3);
  }
  .repo {
    font-weight: 600;
    font-size: var(--text-md);
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: var(--space-3);
    margin: var(--space-4) 0;
    padding: var(--space-3) 0;
    border-block: 1px solid var(--border);
  }
  .facts div {
    display: grid;
  }
  .k {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .v {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .checks {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-2);
  }
  .checks li {
    display: flex;
    gap: var(--space-2);
    align-items: flex-start;
  }
  .checks li :global(.s) {
    margin-top: 2px;
  }
  .cl {
    font-weight: 550;
    font-size: var(--text-sm);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
  }
  .fits td {
    padding: 7px 0;
    border-top: 1px solid var(--border);
  }
  .fits td:last-child {
    text-align: right;
  }
  .fits tr.me td {
    font-weight: 600;
  }
  .fits td.no {
    color: var(--text-faint);
  }
  .grid {
    margin-top: var(--space-3);
  }
  .grid th,
  .grid td {
    padding: 6px;
    text-align: right;
    border-top: 1px solid var(--border);
  }
  .grid th:first-child {
    text-align: left;
  }
  .grid thead th {
    color: var(--text-faint);
    font-weight: 500;
    border-top: none;
  }
  .seg {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 3px;
    border-radius: var(--radius-md);
    background: var(--surface-2);
    border: 1px solid var(--border);
    width: fit-content;
  }
  .seg button {
    height: 30px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .seg button[aria-checked="true"] {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
    font-weight: 550;
  }
  .seg button:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
