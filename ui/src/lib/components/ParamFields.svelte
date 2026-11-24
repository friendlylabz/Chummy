<script lang="ts">
  /**
   * The parameter fields of one block, from its catalog entry. Used by the
   * loop editor's inspector and by Agent Loop Configuration.
   */
  import { onMount } from "svelte";
  import { commands, type BlockInfo, type ParamSpec } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { tools } from "$lib/stores/tools.svelte";
  import Button from "./Button.svelte";
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";
  import { readTools, writeTools, isDisabled, setDisabled, readSwitch } from "$lib/switches";

  let {
    info,
    params,
    onset,
    changed = [],
    onreset,
    agentTools = null,
    idPrefix = "in",
  }: {
    info: BlockInfo;
    params: Record<string, unknown>;
    /** `undefined` removes the parameter (its default applies). */
    onset: (key: string, value: unknown) => void;
    /** Keys that differ from the loop's own settings (Agent Loop Configuration). */
    changed?: string[];
    /** Undo one key's change. */
    onreset?: (key: string) => void;
    /** The agent's own tool switches, to show which tools it may use. */
    agentTools?: string[] | null;
    idPrefix?: string;
  } = $props();

  const roleOptions = ["user", "assistant", "system", "tool"];

  onMount(() => {
    if (info.params.some((p) => p.kind === "tools")) tools.init();
  });

  function value(p: ParamSpec): unknown {
    return params[p.key];
  }
  function roles(): string[] {
    const v = params.roles;
    return Array.isArray(v) ? (v as string[]) : ["user"];
  }
  function toolSwitches() {
    return readTools(params.tools);
  }
  /** Disables every tool in this block (or follows the agent again). */
  function setAllDisabled(off: boolean) {
    onset("tools", writeTools({ all: off ? "off" : "inherit", each: {} }));
  }
  function setToolDisabled(name: string, off: boolean) {
    onset("tools", writeTools(setDisabled(toolSwitches(), name, off)));
  }
  function forget(name: string) {
    const t = toolSwitches();
    const each = { ...t.each };
    delete each[name];
    onset("tools", writeTools({ all: t.all, each }));
  }
  function numberInput(p: ParamSpec, raw: string) {
    if (raw.trim() === "") return onset(p.key, p.optional ? undefined : 0);
    const n = Number(raw);
    if (!isNaN(n)) onset(p.key, p.kind === "integer" ? Math.round(n) : n);
  }
  /** Whether this block offers the tool, and why (or why not). */
  function verdict(t: { enabled: boolean; name: string }): { on: boolean; text: string } {
    if (!settings.current.permissions.tools) return { on: false, text: "tools off in Security and Privacy" };
    if (!t.enabled) return { on: false, text: "off in Settings → Tools" };
    if (isDisabled(toolSwitches(), t.name)) return { on: false, text: "disabled in this block" };
    if (!agentTools) return { on: true, text: "if the agent allows" };
    return agentTools.includes(t.name) ? { on: true, text: "on for the agent" } : { on: false, text: "off for the agent" };
  }
</script>

    {#each info.params as p (p.key)}
      <div class="field" class:changed={changed.includes(p.key)}>
        {#if changed.includes(p.key) && onreset}
          <div class="field-head">
            <span class="hint">Changed for this agent</span>
            <button type="button" class="reset" onclick={() => onreset?.(p.key)}>Use loop default</button>
          </div>
        {/if}
        {#if p.kind === "bool"}
          <label class="check">
            <input type="checkbox" checked={!!value(p)} onchange={(e) => onset(p.key, (e.currentTarget as HTMLInputElement).checked)} />
            {p.label}
          </label>
        {:else}
          <label for="{idPrefix}-{p.key}">{p.label}</label>
          {#if p.kind === "multiline"}
            <textarea
              id="{idPrefix}-{p.key}"
              class="input area"
              rows={p.key === "prompt" || p.key === "format" ? 8 : 4}
              spellcheck="false"
              value={(value(p) as string) ?? ""}
              oninput={(e) => onset(p.key, (e.currentTarget as HTMLTextAreaElement).value)}
            ></textarea>
          {:else if p.kind === "text"}
            <input
              id="{idPrefix}-{p.key}"
              class="input mono"
              spellcheck="false"
              value={(value(p) as string) ?? ""}
              oninput={(e) => onset(p.key, (e.currentTarget as HTMLInputElement).value)}
            />
          {:else if p.kind === "number" || p.kind === "integer"}
            <input
              id="{idPrefix}-{p.key}"
              class="input"
              type="number"
              min={p.min}
              max={p.max}
              step={p.kind === "number" ? p.step : 1}
              placeholder={p.optional ? (p.key === "max_tokens" || p.key === "temperature" ? "Same as the model" : "Default") : ""}
              value={value(p) ?? ""}
              oninput={(e) => numberInput(p, (e.currentTarget as HTMLInputElement).value)}
            />
            {#if p.optional && value(p) != null && (p.key === "max_tokens" || p.key === "temperature")}
              <button type="button" class="reset start" onclick={() => onset(p.key, undefined)}>Use the model's setting</button>
            {/if}
          {:else if p.kind === "choice"}
            <select
              id="{idPrefix}-{p.key}"
              class="input"
              value={p.key === "network" ? readSwitch(value(p)) : ((value(p) as string) ?? p.options[0]?.value)}
              onchange={(e) => onset(p.key, (e.currentTarget as HTMLSelectElement).value)}
            >
              {#each p.options as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
            </select>
          {:else if p.kind === "roles"}
            <div class="roles">
              {#each roles() as r, i (i)}
                <div class="role">
                  <span class="faint xs">{i + 1}.</span>
                  <select class="input" value={r} onchange={(e) => { const v = [...roles()]; v[i] = (e.currentTarget as HTMLSelectElement).value; onset(p.key, v); }}>
                    {#each roleOptions as o (o)}<option value={o}>{o[0].toUpperCase() + o.slice(1)}</option>{/each}
                  </select>
                  {#if roles().length > 1}
                    <button type="button" class="mini" aria-label="Remove" onclick={() => onset(p.key, roles().filter((_, j) => j !== i))}><Icon name="minus" size={13} /></button>
                  {/if}
                </div>
              {/each}
              <button type="button" class="mini add" onclick={() => onset(p.key, [...roles(), "assistant"])}><Icon name="plus" size={13} /> Add role</button>
            </div>
          {:else if p.kind === "tools"}
            {#if !tools.ready}
              <p class="hint">No tools yet. Set them up in Settings → Tools.</p>
              <Button variant="ghost" icon="wrench" onclick={() => commands.openWindow("settings", "tools")}>Open Tools settings</Button>
            {:else}
              <p class="hint">
                Blocks use the agent's tools. Here you can only turn tools off for this block, never on.
              </p>
              <div class="check all-off">
                <Toggle
                  checked={toolSwitches().all === "off"}
                  label="Disable all tools in this block"
                  onchange={(v) => setAllDisabled(v)}
                />
                <span>Disable all tools in this block</span>
              </div>
              <div class="tools">
                {#each tools.installed as t (t.name)}
                  {@const v = verdict(t)}
                  <div class="tool" title={t.description}>
                    <span class="dot" class:on={v.on}></span>
                    <span class="tmain">
                      <span class="tname">{t.title}</span>
                      <span class="off">{v.text}</span>
                    </span>
                    <Toggle
                      checked={isDisabled(toolSwitches(), t.name)}
                      label="Disable {t.title} in this block"
                      onchange={(off) => setToolDisabled(t.name, off)}
                    />
                  </div>
                {/each}
                {#each Object.keys(toolSwitches().each).filter((n) => !tools.installed.some((t) => t.name === n)) as missing (missing)}
                  <div class="tool">
                    <span class="dot"></span><span class="tmain"><span class="tname mono">{missing}</span><span class="off">not installed</span></span>
                    <button type="button" class="mini" aria-label="Forget {missing}" onclick={() => forget(missing)}><Icon name="minus" size={13} /></button>
                  </div>
                {/each}
              </div>
            {/if}
          {:else if p.kind === "model"}
            <select
              id="{idPrefix}-{p.key}"
              class="input"
              value={(value(p) as string) ?? ""}
              onchange={(e) => onset(p.key, (e.currentTarget as HTMLSelectElement).value || undefined)}
            >
              <option value="">The agent's model</option>
              {#each settings.current.providers as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
            </select>
          {/if}
        {/if}
        {#if p.help}<span class="hint">{p.help}</span>{/if}
      </div>
    {/each}

<style>
  .field {
    display: grid;
    gap: 5px;
    margin-bottom: var(--space-4);
  }
  .field.changed {
    padding-left: var(--space-2);
    border-left: 2px solid var(--accent);
  }
  .field-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .field-head .hint {
    flex: 1;
  }
  .reset {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .reset.start {
    justify-self: start;
  }
  label {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .check {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    font-weight: 400;
  }
  .hint {
    color: var(--text-faint);
    font-size: var(--text-xs);
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
  .input:focus {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border-strong));
  }
  .input.mono,
  .area {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }
  .area {
    height: auto;
    padding: var(--space-2) var(--space-3);
    resize: vertical;
    line-height: 1.5;
  }
  .roles,
  .all-off {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: var(--space-2) 0;
  }
  .tools {
    display: grid;
    gap: 4px;
  }
  .role {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .mini {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .mini.add {
    justify-self: start;
  }
  .tname {
    flex: 1;
  }
  .tools {
    margin-top: var(--space-1);
  }
  .tool {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 3px 0;
    font-size: var(--text-sm);
  }
  .tmain {
    display: grid;
    flex: 1;
    min-width: 0;
    line-height: 1.3;
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--border-strong);
  }
  .dot.on {
    background: var(--accent);
  }
  .off {
    color: var(--text-faint);
    font-size: var(--text-xs);
  }
  .faint {
    color: var(--text-faint);
  }
  .xs {
    font-size: var(--text-xs);
  }
  .mono {
    font-family: var(--font-mono);
  }
</style>
