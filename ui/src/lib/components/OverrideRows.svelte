<script lang="ts">
  /**
   * Generation settings that inherit unless changed: for each one (temperature,
   * longest answer, reasoning effort, sampling…), **Inherit** the value from
   * above, **Set** one here, or **Don't send** it (the server's default).
   * Used for an agent's model settings and for a model block's own settings.
   */
  import { untrack } from "svelte";
  import type { ProviderKind } from "$lib/ipc";
  import { describe, EFFORTS, parseParam, SETTINGS, showParam, unsupported, type ModelOverrides, type SettingKey } from "$lib/generation";

  type Mode = "inherit" | "set" | "off";

  let {
    value,
    onchange,
    bad = $bindable(false),
    kind,
    inherited,
    from = "Model",
    noOff = [],
    compact = false,
    idPrefix = "ov",
  }: {
    value: ModelOverrides;
    /** Called with the new overrides whenever they change and are valid. */
    onchange: (o: ModelOverrides) => void;
    bad?: boolean;
    /** The model's kind of server, to grey out what it doesn't take. */
    kind?: ProviderKind;
    /** What a setting is when inherited (undefined: unknown here). */
    inherited?: (k: SettingKey) => unknown;
    /** Where inherited values come from, in words. */
    from?: string;
    /** Settings that can only be inherited or set. */
    noOff?: SettingKey[];
    /** Narrow layout (the loop editor's inspector). */
    compact?: boolean;
    idPrefix?: string;
  } = $props();

  /** The same settings always compare equal, whatever their key order. */
  const canon = (o: ModelOverrides) =>
    JSON.stringify(Object.fromEntries(Object.entries(o ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))));

  const modes = $state<Record<string, Mode>>({});
  const texts = $state<Record<string, string>>({});
  let last = "";

  function load(o: ModelOverrides) {
    for (const r of SETTINGS) {
      const has = Object.prototype.hasOwnProperty.call(o, r.key);
      const v = o[r.key];
      const unset = v === null || (Array.isArray(v) && !v.length);
      modes[r.key] = !has ? "inherit" : unset ? "off" : "set";
      texts[r.key] = has && !unset ? showParam(r.type, v) : "";
    }
    last = canon(o);
  }
  // Load again when the value changes from outside (another block, undo, reset).
  $effect(() => {
    const now = canon(value ?? {});
    untrack(() => {
      if (now !== last) load(value ?? {});
    });
  });

  const rows = $derived(
    SETTINGS.map((r) => {
      const mode = modes[r.key] ?? "inherit";
      const res = mode === "set" ? parseParam(r, texts[r.key] ?? "") : null;
      const missing = mode === "set" && !!res && "value" in res && (res.value === null || (Array.isArray(res.value) && !res.value.length));
      return { r, mode, res, missing };
    }),
  );
  const options = $derived.by(() => {
    const o: Record<string, unknown> = {};
    for (const { r, mode, res } of rows) {
      if (mode === "off") o[r.key] = r.type === "stop" ? [] : null;
      else if (mode === "set" && res && "value" in res && res.value !== null) o[r.key] = res.value;
    }
    return o as ModelOverrides;
  });
  $effect(() => {
    const wrong = rows.some(({ res, missing }) => (res && "error" in res) || missing);
    bad = wrong;
    const now = canon(options);
    if (!wrong && now !== last) {
      last = now;
      untrack(() => onchange(options));
    }
  });

  function setMode(k: SettingKey, m: Mode) {
    modes[k] = m;
    // Start from the inherited value, to edit it.
    if (m === "set" && !(texts[k] ?? "").trim()) {
      const row = SETTINGS.find((r) => r.key === k)!;
      const v = inherited?.(k);
      texts[k] = v === null || v === undefined ? "" : showParam(row.type, v);
    }
  }
  function sub(k: SettingKey, type: (typeof SETTINGS)[number]["type"]): string {
    const v = inherited?.(k);
    return v === undefined ? `${from}'s setting` : `${from}: ${describe(type, v)}`;
  }
</script>

<div class="rows" class:compact>
  {#each rows as { r, mode, res, missing } (r.key)}
    {@const why = kind ? unsupported(kind, r.key) : null}
    {@const id = `${idPrefix}-${r.key}`}
    <div class="row" class:changed={mode !== "inherit"} class:off={!!why}>
      <div class="name">
        <label class="row-title" for={id}>{r.label}</label>
        <span class="row-sub" title={r.help}>{why ?? sub(r.key, r.type)}</span>
      </div>
      <select
        {id}
        class="input mode"
        value={mode}
        onchange={(e) => setMode(r.key, e.currentTarget.value as Mode)}
        aria-label={`${r.label}: inherit, set or don't send`}
      >
        <option value="inherit">Inherit</option>
        <option value="set">Set</option>
        {#if !r.required && !noOff.includes(r.key)}<option value="off">Don't send</option>{/if}
      </select>
      {#if mode === "set" || !compact}
        <div class="value">
          {#if mode === "set"}
            {#if r.type === "effort"}
              <select class="input" bind:value={texts[r.key]} aria-label={r.label}>
                <option value="" disabled>Choose…</option>
                {#each EFFORTS as e (e.value)}<option value={e.value}>{e.label}</option>{/each}
              </select>
            {:else if r.type === "stop"}
              <textarea class="input mono" rows="2" bind:value={texts[r.key]} placeholder="One per line" aria-label={r.label} spellcheck="false"></textarea>
            {:else}
              <input
                class="input"
                bind:value={texts[r.key]}
                inputmode={r.type === "int" ? "numeric" : "decimal"}
                placeholder={r.placeholder}
                aria-label={r.label}
                aria-invalid={(res && "error" in res) || missing}
              />
            {/if}
            {#if res && "error" in res}<span class="err">{res.error}</span>{:else if missing}<span class="err">Enter a value, or choose Inherit.</span>{/if}
          {:else if mode === "off"}
            <span class="muted small">Not sent: the server's default.</span>
          {:else}
            <span class="muted small" title={r.help}>{r.help}</span>
          {/if}
        </div>
      {/if}
    </div>
  {/each}
</div>

<style>
  .rows {
    display: grid;
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(150px, 1.1fr) 120px minmax(160px, 1.4fr);
    gap: var(--space-3);
    align-items: center;
    padding: var(--space-2) var(--space-3);
  }
  .row + .row {
    border-top: 1px solid var(--border);
  }
  .row.changed {
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }
  .row.off .row-title {
    color: var(--text-faint);
  }
  .name {
    display: grid;
    min-width: 0;
  }
  .row-title {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .row-sub {
    color: var(--text-muted);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .value {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .value .input,
  .mode {
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
  }
  .value .small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .err {
    color: var(--danger);
    font-size: var(--text-xs);
  }
  textarea {
    resize: vertical;
  }
  /* The inspector is narrow: name and choice on one line, the value below. */
  .compact .row {
    grid-template-columns: minmax(0, 1fr) 104px;
    gap: var(--space-1) var(--space-2);
    padding: var(--space-2);
  }
  .compact .value {
    grid-column: 1 / -1;
  }
  .compact .mode {
    font-size: var(--text-xs);
  }
</style>
