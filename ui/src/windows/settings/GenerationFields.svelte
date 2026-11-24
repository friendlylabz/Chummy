<script lang="ts">
  /**
   * A model connection's optional generation settings: reasoning effort,
   * sampling, penalties, seed and stop sequences. Empty means "not sent", so
   * the server's default applies. Settings this kind of server doesn't take
   * are greyed out with the reason (and never sent).
   */
  import { untrack } from "svelte";
  import type { ProviderKind } from "$lib/ipc";
  import { EFFORTS, PARAMS, parseParam, showParam, unsupported, type Generation } from "$lib/generation";
  import Icon from "$lib/components/Icon.svelte";

  let {
    kind,
    value = $bindable({}),
    bad = $bindable(false),
  }: { kind: ProviderKind; value?: Generation; bad?: boolean } = $props();

  const start = untrack(() => value) ?? {};
  const texts = $state<Record<string, string>>(
    Object.fromEntries(PARAMS.map((p) => [p.key, showParam(p.type, start[p.key])])),
  );
  const results = $derived(PARAMS.map((p) => ({ p, r: parseParam(p, texts[p.key]) })));
  const anySet = PARAMS.some((p) => texts[p.key].trim() !== "");
  // Open when something's set, so it isn't hidden.
  let open = $state(anySet);
  const count = $derived(results.filter(({ r }) => "value" in r && r.value !== null && !(Array.isArray(r.value) && !r.value.length)).length);

  $effect(() => {
    const g: Record<string, unknown> = {};
    let wrong = false;
    for (const { p, r } of results) {
      if ("error" in r) wrong = true;
      else if (r.value !== null && !(Array.isArray(r.value) && !r.value.length)) g[p.key] = r.value;
    }
    value = g as Generation;
    bad = wrong;
  });
</script>

<details class="more" bind:open>
  <summary>
    <span class="chev"><Icon name="chevron-down" size={14} /></span>
    More generation settings
    <span class="muted small">{count ? `${count} set` : "server defaults"}</span>
  </summary>
  <p class="hint intro">Empty means not sent, so the server's own default applies. An agent can change any of these for itself (Settings → Agents).</p>
  <div class="grid">
    {#each results as { p, r } (p.key)}
      {@const why = unsupported(kind, p.key)}
      <div class="field" class:off={!!why} class:wide={p.type === "stop" || p.type === "effort"}>
        <label for={`gen-${p.key}`}>{p.label}</label>
        {#if p.type === "effort"}
          <select id={`gen-${p.key}`} class="input" bind:value={texts[p.key]} disabled={!!why}>
            <option value="">Server default</option>
            {#each EFFORTS as e (e.value)}<option value={e.value}>{e.label}</option>{/each}
          </select>
        {:else if p.type === "stop"}
          <textarea id={`gen-${p.key}`} class="input mono" rows="2" bind:value={texts[p.key]} disabled={!!why} placeholder="None" spellcheck="false"
          ></textarea>
        {:else}
          <input
            id={`gen-${p.key}`}
            class="input"
            bind:value={texts[p.key]}
            inputmode={p.type === "int" ? "numeric" : "decimal"}
            placeholder={p.placeholder}
            disabled={!!why}
            aria-invalid={"error" in r}
          />
        {/if}
        <span class="hint" class:err={"error" in r}>{"error" in r ? r.error : (why ?? p.help)}</span>
      </div>
    {/each}
  </div>
</details>

<style>
  .more {
    margin-top: var(--space-4);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border);
  }
  summary {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 600;
    font-size: var(--text-sm);
    cursor: pointer;
    list-style: none;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  .chev {
    display: inline-flex;
    color: var(--text-muted);
    transform: rotate(-90deg);
    transition: transform var(--dur-fast) var(--ease);
  }
  .more[open] .chev {
    transform: none;
  }
  .intro {
    display: block;
    margin: var(--space-2) 0 var(--space-3);
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: var(--space-3) var(--space-4);
  }
  .grid .field {
    margin: 0;
    /* A shorter cell mustn't spread its rows and push its input down. */
    align-content: start;
  }
  .wide {
    grid-column: 1 / -1;
  }
  .wide select {
    max-width: 260px;
  }
  .off label {
    color: var(--text-faint);
  }
  .err {
    color: var(--danger);
  }
  textarea {
    resize: vertical;
    max-width: 420px;
  }
</style>
