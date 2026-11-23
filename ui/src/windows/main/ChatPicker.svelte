<script lang="ts">
  /**
   * The active agent's chats, next to the agent picker: New chat first,
   * then every chat, newest first, with a dot on the ones working.
   */
  import { chat } from "$lib/stores/chat.svelte";
  import { runs } from "$lib/stores/runs.svelte";
  import { setup } from "$lib/stores/setup.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let open = $state(false);
  const current = $derived(chat.current);
  const usable = $derived(setup.ready && !!chat.agentId);

  function when(unix: number): string {
    if (!unix) return "";
    const d = new Date(unix * 1000);
    const today = new Date();
    if (d.toDateString() === today.toDateString()) {
      return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    }
    return d.toLocaleDateString([], { month: "short", day: "numeric" });
  }

  function choose(id: string) {
    open = false;
    chat.choose(id);
  }

  function create() {
    open = false;
    chat.create();
  }
</script>

<svelte:window onkeydown={(e) => open && e.key === "Escape" && (open = false)} />

<div class="picker">
  <button class="pill" type="button" aria-haspopup="listbox" aria-expanded={open} disabled={!usable} onclick={() => (open = !open)}>
    <Icon name="chat" size={15} />
    <span class="label">{usable ? (current?.title ?? "New chat") : "No chat"}</span>
    {#if usable}<Icon name="chevron-down" size={14} />{/if}
  </button>

  {#if open}
    <div class="scrim" onclick={() => (open = false)} aria-hidden="true"></div>
    <div class="menu" role="listbox" aria-label="Chats">
      <button type="button" class="new" onclick={create}>
        <Icon name="plus" size={15} />
        <span class="name">New chat</span>
      </button>
      <div class="sep"></div>
      <div class="list">
        {#each chat.chats as c (c.id)}
          {@const working = !!runs.forChat(c.id)}
          <div class="row" class:current={c.id === current?.id}>
            <button type="button" role="option" aria-selected={c.id === current?.id} class="pick" onclick={() => choose(c.id)}>
              <span class="name">{c.title}</span>
              {#if working}<span class="dot" title="Working"></span>{:else}<span class="time">{when(c.updated_at)}</span>{/if}
              {#if c.id === current?.id}<Icon name="check" size={15} />{/if}
            </button>
            {#if !working}
              <button type="button" class="del" aria-label={`Delete “${c.title}”`} title="Delete chat" onclick={() => chat.remove(c.id)}>
                <Icon name="trash" size={13} />
              </button>
            {/if}
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    min-width: 0;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 280px;
    height: 30px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-full);
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text);
    font-size: var(--text-sm);
    font-weight: 550;
    cursor: pointer;
  }
  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill:disabled {
    color: var(--text-muted);
    cursor: default;
  }
  .pill:not(:disabled):hover {
    border-color: var(--border-strong);
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 10;
  }
  .menu {
    position: absolute;
    top: 36px;
    left: 0;
    z-index: 11;
    width: 300px;
    padding: var(--space-1);
    border-radius: var(--radius-lg);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
  }
  .list {
    max-height: 360px;
    overflow-y: auto;
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 34px;
    padding: 0 var(--space-3);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .new {
    width: 100%;
    color: var(--accent) !important;
    font-weight: 550;
  }
  .menu button:hover {
    background: var(--surface-hover);
  }
  .row {
    display: flex;
    align-items: center;
    border-radius: var(--radius-md);
  }
  .row:hover {
    background: var(--surface-hover);
  }
  .row .pick {
    flex: 1;
    min-width: 0;
  }
  .row .pick:hover,
  .row .del:hover {
    background: transparent;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .time {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .del {
    width: 30px;
    justify-content: center;
    padding: 0 !important;
    color: var(--text-faint) !important;
    opacity: 0;
  }
  .row:hover .del,
  .del:focus-visible {
    opacity: 1;
  }
  .del:hover {
    color: var(--danger) !important;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .sep {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--border);
  }
</style>
