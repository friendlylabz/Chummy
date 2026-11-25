<script lang="ts">
  import { onMount } from "svelte";
  import { commands, type Template } from "$lib/ipc";
  import { date } from "$lib/format";
  import { loops } from "$lib/stores/loops.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let {
    onopen,
    ontemplate,
    onimport,
  }: { onopen: (id: string) => void; ontemplate: (t: Template) => void; onimport: () => void } = $props();

  let templates = $state<Template[]>([]);
  onMount(async () => (templates = await commands.loopTemplates()));
</script>

<div class="start">
  <div class="inner">
    <header>
      <div>
        <h1>Loops</h1>
        <p class="sub">A loop is how an agent thinks: blocks that pass text to each other, from your message to its reply.</p>
        <p class="sub">Loops from elsewhere are programs: Chummy shows you a review of each one before it can run. Only import loops from people you trust.</p>
      </div>
      <Button icon="import" onclick={onimport}>Import…</Button>
    </header>

    {#if loops.list.length}
      <div class="label">Your loops</div>
      <div class="list">
        {#each loops.list as l (l.id)}
          <div class="row">
            <button class="open" type="button" onclick={() => onopen(l.id)} title={l.trusted ? undefined : "Not on your list of trusted loops: review it before it can run or be edited"}>
              <span class="icon" class:untrusted={!l.trusted}><Icon name={l.trusted ? "flow" : "shield"} size={17} /></span>
              <span class="text">
                <span class="name">{l.name} {#if !l.trusted}<span class="badge warn">Needs a review</span>{:else if !l.ok}<span class="badge warn">Has problems</span>{/if}</span>
                <span class="meta">{l.description || `${l.blocks} blocks`} · edited {date(l.modified)}</span>
              </span>
            </button>
            <Button variant="ghost" icon="export" iconOnly aria-label="Export" title="Export" onclick={() => commands.exportLoop(l.id)} />
            <Button variant="ghost" icon="trash" iconOnly aria-label="Delete" title="Delete" onclick={() => commands.deleteLoop(l.id)} />
          </div>
        {/each}
      </div>
    {/if}

    <div class="label">Start something new</div>
    <div class="templates">
      {#each templates as t (t.id)}
        <button class="tpl" type="button" onclick={() => ontemplate(t)}>
          <span class="name">{t.name}</span>
          <span class="meta">{t.description}</span>
          <span class="count">{t.file.blocks.length} blocks</span>
        </button>
      {/each}
    </div>
  </div>
</div>

<style>
  .start {
    flex: 1;
    overflow-y: auto;
  }
  .inner {
    max-width: 780px;
    margin: 0 auto;
    padding: var(--space-7) var(--space-6);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-4);
  }
  h1 {
    font-size: var(--text-2xl);
    font-weight: 650;
    letter-spacing: -0.02em;
  }
  .sub {
    margin-top: var(--space-1);
    color: var(--text-muted);
    max-width: 56ch;
  }
  .label {
    margin: var(--space-6) 0 var(--space-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .list {
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 2px;
    padding-right: var(--space-2);
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border: none;
    background: transparent;
    text-align: left;
    cursor: pointer;
  }
  .open:hover .name {
    color: var(--accent);
  }
  .icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 34px;
    height: 34px;
    border-radius: var(--radius-md);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .icon.untrusted {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .text {
    display: grid;
    min-width: 0;
  }
  .name {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 600;
  }
  .meta {
    color: var(--text-muted);
    font-size: var(--text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    padding: 0 var(--space-2);
    border-radius: var(--radius-full);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .badge.warn {
    background: color-mix(in srgb, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .templates {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: var(--space-3);
  }
  .tpl {
    display: grid;
    gap: 4px;
    align-content: start;
    padding: var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    text-align: left;
    cursor: pointer;
    transition: border-color var(--dur-fast) var(--ease);
  }
  .tpl:hover {
    border-color: var(--accent);
  }
  .tpl .meta {
    white-space: normal;
  }
  .count {
    margin-top: var(--space-2);
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
</style>
