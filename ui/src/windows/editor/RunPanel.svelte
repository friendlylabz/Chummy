<script lang="ts">
  import { commands, errorText, type Block } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import type { LoopFile } from "$lib/ipc";

  let {
    file,
    canRun,
    runId = $bindable(null),
    onclose,
  }: { file: () => LoopFile; canRun: boolean; runId: string | null; onclose: () => void } = $props();

  let agentId = $state(settings.current.active_agent ?? "");
  let message = $state("");
  let overrides = $state<Record<string, string>>({});
  let error = $state("");

  const inputs = $derived(file().blocks.filter((b: Block) => b.type === "input"));
  const run = $derived(runId ? runs.runs[runId] : undefined);
  const running = $derived(!!run && !run.outcome);

  async function start() {
    error = "";
    try {
      const o = Object.fromEntries(Object.entries(overrides).filter(([, v]) => v.trim() !== ""));
      runId = await commands.testRun(file(), agentId || null, message, o);
    } catch (e) {
      error = errorText(e);
    }
  }

  function statusText() {
    const o = run?.outcome;
    if (!o) return "";
    switch (o.status.status) {
      case "completed":
        return `Finished: ${o.steps} steps, ${o.llm_calls} model call${o.llm_calls === 1 ? "" : "s"}${o.tool_calls ? `, ${o.tool_calls} tool call${o.tool_calls === 1 ? "" : "s"}` : ""}.`;
      case "stopped":
        return "Stopped.";
      case "limit_reached":
        return o.status.message;
      case "failed":
        return o.status.error;
    }
  }
</script>

<aside class="run">
  <div class="head">
    <span class="title">Test run</span>
    <button class="x" type="button" aria-label="Close" onclick={onclose}><Icon name="close" size={15} /></button>
  </div>

  <div class="field">
    <label for="rp-agent">Run as</label>
    <select id="rp-agent" class="input" bind:value={agentId}>
      {#each settings.current.agents as a (a.id)}<option value={a.id}>{a.name} (its model and workspace)</option>{/each}
      <option value="">No agent (default model, no workspace)</option>
    </select>
  </div>

  <div class="field">
    <label for="rp-msg">Message</label>
    <textarea id="rp-msg" class="input area" rows="4" bind:value={message} placeholder="What every Message block sends"></textarea>
  </div>
  {#if inputs.length > 1}
    {#each inputs as b (b.id)}
      <div class="field">
        <label for="rp-{b.id}">{b.label || b.id} <span class="faint">(optional)</span></label>
        <textarea id="rp-{b.id}" class="input area" rows="2" bind:value={overrides[b.id]} placeholder="Same as Message"></textarea>
      </div>
    {/each}
  {/if}

  <div class="actions">
    {#if running}
      <Button icon="stop" onclick={() => commands.stopRun(runId!)}>Stop</Button>
      <span class="live"><span class="pulse"></span>{run?.currentLabel || "Starting"}…</span>
    {:else}
      <Button variant="primary" icon="play" onclick={start} disabled={!canRun}>Run</Button>
      {#if !canRun}<span class="faint small">Fix the errors first.</span>{/if}
    {/if}
  </div>
  {#if error}<p class="err">{error}</p>{/if}

  {#if run}
    {#if run.streaming && running}
      <div class="section">Model output</div>
      <div class="box stream">{run.streaming.slice(-1200)}</div>
    {/if}
    {#if run.activity.length}
      <div class="section">Tools and notices</div>
      <ul class="activity">
        {#each run.activity.slice(-40) as a, i (i)}
          <li class={a.kind}>
            <span class="tag">{a.kind === "call" ? "Call" : a.kind === "result" ? "Result" : a.kind === "error" ? "Error" : "Note"}</span>
            <span class="mono">{a.text}</span>
          </li>
        {/each}
      </ul>
    {/if}
    <div class="section">Reply</div>
    {#if run.replies.length}
      <div class="box" data-selectable>{run.replies.join("\n\n")}</div>
    {:else}
      <p class="faint small">{running ? "Nothing yet." : "No reply."}</p>
    {/if}
    {#if run.outcome}
      <p class="status" class:bad={run.outcome.status.status !== "completed"}>{statusText()}</p>
      {#if run.outcome.waiting.length}
        <p class="faint small">Still waiting for inputs: {run.outcome.waiting.join(", ")}</p>
      {/if}
    {/if}
  {/if}
</aside>

<style>
  .activity {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
    font-size: var(--text-xs);
  }
  .activity li {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
    word-break: break-word;
  }
  .activity .mono {
    font-family: var(--font-mono);
    color: var(--text-muted);
  }
  .tag {
    flex: none;
    width: 46px;
    color: var(--text-faint);
    font-weight: 600;
  }
  .activity .error .tag {
    color: var(--danger);
  }
  .activity .call .tag {
    color: var(--accent);
  }
  .run {
    flex: none;
    width: 300px;
    overflow-y: auto;
    padding: var(--space-4);
    border-left: 1px solid var(--border);
    background: var(--surface);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-3);
  }
  .title {
    font-weight: 650;
  }
  .x {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .x:hover {
    background: var(--surface-hover);
  }
  .field {
    display: grid;
    gap: 5px;
    margin-bottom: var(--space-3);
  }
  label {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .input {
    width: 100%;
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    outline: none;
    font-size: var(--text-sm);
  }
  .area {
    height: auto;
    padding: var(--space-2) var(--space-3);
    resize: vertical;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }
  .live {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .pulse {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .section {
    margin: var(--space-4) 0 var(--space-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .box {
    padding: var(--space-3);
    border-radius: var(--radius-md);
    background: var(--bg);
    border: 1px solid var(--border);
    font-size: var(--text-sm);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 280px;
    overflow-y: auto;
  }
  .stream {
    color: var(--text-muted);
    font-size: var(--text-xs);
    max-height: 140px;
  }
  .status {
    margin-top: var(--space-3);
    font-size: var(--text-sm);
    color: var(--ok);
  }
  .status.bad {
    color: var(--warn);
  }
  .err {
    margin-top: var(--space-2);
    color: var(--danger);
    font-size: var(--text-sm);
  }
  .faint {
    color: var(--text-faint);
  }
  .small {
    font-size: var(--text-sm);
  }
</style>
