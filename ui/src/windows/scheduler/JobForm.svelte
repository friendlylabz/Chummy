<script lang="ts">
  import type { Job, Trigger } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { loops } from "$lib/stores/loops.svelte";
  import Button from "$lib/components/Button.svelte";
  import TriggerRow from "./TriggerRow.svelte";

  let { draft = $bindable() }: { draft: Job } = $props();

  const agents = $derived(settings.current.agents);
  const agentMissing = $derived(!agents.some((a) => a.id === draft.agent));

  function today(h = 9): string {
    const d = new Date();
    d.setDate(d.getDate() + 1);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(h)}:00`;
  }

  const adders: { label: string; make: () => Trigger }[] = [
    { label: "At a time", make: () => ({ kind: "daily", time: "09:00", days: [] }) },
    { label: "Repeating", make: () => ({ kind: "every", minutes: 60 }) },
    { label: "Once", make: () => ({ kind: "once", at: today() }) },
    { label: "When files change", make: () => ({ kind: "folder", path: "inbox" }) },
  ];

  function opt(v: string): string | null {
    const t = v.trim();
    return t ? t : null;
  }
</script>

<div class="form">
  <div class="field">
    <label for="j-name">Name</label>
    <input id="j-name" class="input" bind:value={draft.name} placeholder="Morning digest" maxlength="100" />
  </div>

  <div class="two">
    <div class="field">
      <label for="j-agent">Runs as</label>
      <select id="j-agent" class="input" bind:value={draft.agent}>
        {#if agentMissing}<option value={draft.agent}>Choose an agent…</option>{/if}
        {#each agents as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
      </select>
      <div class="help">It uses this agent's folder, model and permissions. A job can't do anything the agent can't.</div>
    </div>
    <div class="field">
      <label for="j-loop">Loop</label>
      <select
        id="j-loop"
        class="input"
        value={draft.loop ?? ""}
        onchange={(e) => (draft.loop = e.currentTarget.value || null)}
      >
        <option value="">The agent's own loop</option>
        {#each loops.list as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
      </select>
    </div>
  </div>

  <div class="field">
    <label for="j-msg">Message</label>
    <textarea id="j-msg" class="input area" rows="4" bind:value={draft.message} placeholder="Summarize what changed in notes/ today."></textarea>
    <div class="help">
      What the loop starts with, like a chat message. <code>_NOW_</code>, <code>_DATE_</code> and <code>_TIME_</code> become
      the date and time; <code>_FILE_</code> and <code>_FILE_TEXT_</code> the file's path and text.
    </div>
  </div>

  <div class="field">
    <label for="j-file">Include a file <span class="faint">(optional)</span></label>
    <input
      id="j-file"
      class="input mono"
      value={draft.file ?? ""}
      oninput={(e) => (draft.file = opt(e.currentTarget.value))}
      placeholder="todo.md"
    />
    <div class="help">A file in the agent's folder, read when the job runs. Its text is added after the message unless you place it with _FILE_TEXT_.</div>
  </div>

  <div class="field">
    <span class="label">When it runs</span>
    <div class="triggers">
      {#each draft.triggers as t, i (t)}
        <TriggerRow bind:trigger={draft.triggers[i]} onremove={() => draft.triggers.splice(i, 1)} />
      {:else}
        <div class="help">No schedule: it only runs when you press Run now.</div>
      {/each}
    </div>
    {#if draft.triggers.length < 10}
      <div class="adders">
        {#each adders as a (a.label)}
          <Button variant="ghost" icon="plus" onclick={() => draft.triggers.push(a.make())}>{a.label}</Button>
        {/each}
      </div>
    {/if}
  </div>

  <div class="two">
    <div class="field">
      <label for="j-missed">If a time is missed</label>
      <select id="j-missed" class="input" bind:value={draft.missed}>
        <option value="skip">Skip it</option>
        <option value="run_once">Run once when Chummy is back</option>
      </select>
      <div class="help">Times pass unrun while Chummy is closed or the computer sleeps.</div>
    </div>
    <div class="field">
      <label for="j-limit">Time limit (minutes, 0 = none)</label>
      <input id="j-limit" class="input" type="number" min="0" max="10080" bind:value={draft.time_limit_minutes} />
    </div>
  </div>

  <div class="two">
    <div class="field">
      <label for="j-save">Save the reply to <span class="faint">(optional)</span></label>
      <input
        id="j-save"
        class="input mono"
        value={draft.save_reply_to ?? ""}
        oninput={(e) => (draft.save_reply_to = opt(e.currentTarget.value))}
        placeholder="reports/_DATE_.md"
      />
    </div>
    <div class="field">
      <label for="j-notify">Notify me</label>
      <select id="j-notify" class="input" bind:value={draft.notify}>
        <option value="problems">If it doesn't finish</option>
        <option value="always">Every time it runs</option>
        <option value="never">Never</option>
      </select>
    </div>
  </div>
</div>

<style>
  .form {
    display: grid;
    gap: var(--space-4);
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-4);
  }
  .field {
    display: grid;
    gap: var(--space-1);
    align-content: start;
  }
  label,
  .label {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .faint {
    color: var(--text-faint);
    font-weight: 400;
  }
  .help {
    color: var(--text-faint);
    font-size: var(--text-xs);
    line-height: 1.5;
  }
  .help code {
    font-family: var(--font-mono);
  }
  .input {
    width: 100%;
    height: 36px;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
  }
  .input.area {
    height: auto;
    padding: var(--space-2) var(--space-3);
    resize: vertical;
    line-height: 1.5;
  }
  .input.mono {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
  }
  .triggers {
    display: grid;
    gap: var(--space-2);
  }
  .adders {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
</style>
