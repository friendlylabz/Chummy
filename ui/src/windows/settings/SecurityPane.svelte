<script lang="ts">
  import { onMount } from "svelte";
  import { commands, errorText, type ContainerEngine, type ProgramFor, type SandboxKind } from "$lib/ipc";
  import { settings } from "$lib/stores/settings.svelte";
  import { sandbox } from "$lib/stores/sandbox.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import Button from "$lib/components/Button.svelte";
  import Icon from "$lib/components/Icon.svelte";

  let { gotoTools }: { gotoTools: () => void } = $props();

  const perms = $derived(settings.current.permissions);
  const st = $derived(sandbox.status);
  const cfg = $derived(settings.current.sandbox);
  const health = $derived(st?.health);

  let error = $state("");
  let busy = $state<string | null>(null);
  let image = $state("");
  let osImage = $state("");
  let removed = $state<number | null>(null);

  onMount(() => sandbox.init());

  // Keep the text boxes in step with saved settings (when not being edited).
  let lastImage = "";
  let lastOs = "";
  $effect(() => {
    if (cfg.container.image !== lastImage) image = lastImage = cfg.container.image;
    if (cfg.openshell.image !== lastOs) osImage = lastOs = cfg.openshell.image;
  });

  const blurbs: Record<SandboxKind, string> = {
    native: "Built in. Python compiled to WebAssembly: nothing to install, works the same everywhere. Tools can't start other programs.",
    container:
      "A fresh Docker or Podman container for each call, with no network. Tools can run the image's programs. Needs Docker or Podman installed.",
    openshell:
      "NVIDIA OpenShell sandboxes with Landlock and no network. Tools can run programs. Needs OpenShell and a gateway; the workspace is copied in and out for each call.",
  };

  async function act(name: string, f: () => Promise<unknown>) {
    error = "";
    busy = name;
    try {
      await f();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = null;
    }
  }

  const choose = (k: SandboxKind) => act("kind", () => commands.setSandboxKind(k));
  const check = () => act("check", () => commands.checkSandbox());

  function saveContainer(patch: Partial<{ engine: ContainerEngine; image: string; memory_mb: number; cpus: number }>) {
    const c = cfg.container;
    return act("container", () =>
      commands.setContainerOptions({
        engine: c.engine,
        image: c.image,
        memory_mb: c.memory_mb,
        cpus: c.cpus,
        ...patch,
      }),
    );
  }

  const program = (which: ProgramFor) => act("program", () => commands.chooseSandboxProgram(which));
  const autoProgram = (which: ProgramFor) => act("program", () => commands.clearSandboxProgram(which));
  const getImage = (pull: boolean) => act(pull ? "pull" : "local", () => commands.getContainerImage(pull));
  const saveOsImage = () => act("os", () => commands.setOpenshellImage(osImage));
  const removeAll = () =>
    act("remove", async () => {
      removed = await commands.removeOpenshellSandboxes();
    });

  const short = (id: string) => id.replace(/^sha256:/, "sha256:").slice(0, 19);
  const engineLabel = $derived(cfg.container.engine === "podman" ? "Podman" : "Docker");
</script>

<h1>Security and Privacy</h1>
<p class="pane-sub">
  Everything beyond the workspace folder is off until you turn it on. Turning something on asks you to confirm.
</p>

<div class="section-label">Permissions</div>
<div class="card rows">
  <div class="row">
    <div>
      <div class="row-title">Internet access</div>
      <div class="row-sub">The app-wide switch. A tool goes online only if this, the tool's internet switch in Tools, the agent's internet switch in Agents, and the block's “may use the internet” are all on.</div>
    </div>
    <Toggle checked={perms.network} label="Internet access" onchange={(v) => commands.setPermission("network", v)} />
  </div>
  <div class="row">
    <div>
      <div class="row-title">Tools</div>
      <div class="row-sub">The app-wide switch. A tool runs only if this and the tool are on in Tools, the agent may use it (Agents), and the loop block ticks it. Tools stay locked to the agent's workspace.</div>
    </div>
    <Toggle checked={perms.tools} label="Tools" onchange={(v) => commands.setPermission("tools", v)} />
  </div>
  <div class="row">
    <div>
      <div class="row-title">Ask before tools go online</div>
      <div class="row-sub">
        Every call to a tool that can use the internet waits for your OK in chat, so a web page can't quietly talk an
        agent into sending your files somewhere. Scheduled jobs wait too.
      </div>
    </div>
    <Toggle
      checked={perms.confirm_online_tools}
      label="Ask before tools go online"
      onchange={(v) => commands.setPermission("confirm_online_tools", v)}
    />
  </div>
</div>

<div class="section-label">Sandbox</div>
<p class="muted small intro">
  Where tool calls run. Whichever you choose, a tool sees only the agent's workspace, has no network of its own, and
  goes online only through Chummy with the switches above. Your loops and models always run in Chummy itself.
</p>

{#if error}
  <div class="notice danger" role="alert" style="margin-bottom: var(--space-3)">
    <Icon name="x-circle" size={16} /><span>{error}</span>
  </div>
{/if}

{#if st}
  <div class="kinds" role="radiogroup" aria-label="Sandbox">
    {#each st.kinds as k (k.kind)}
      <button
        type="button"
        role="radio"
        aria-checked={cfg.kind === k.kind}
        class="kind"
        class:on={cfg.kind === k.kind}
        disabled={busy === "kind"}
        onclick={() => choose(k.kind)}
      >
        <span class="dot" aria-hidden="true"></span>
        <span class="k-text">
          <span class="row-title">
            {k.label}
            {#if k.kind === "native"}<span class="tag">Default</span>{/if}
          </span>
          <span class="row-sub">{blurbs[k.kind]}</span>
        </span>
      </button>
    {/each}
  </div>

  {#if health}
    <div class="card card-pad status" class:bad={!health.ready}>
      <div class="status-head">
        <Icon name={st.checking ? "refresh" : health.ready ? "check" : "alert"} size={16} />
        <span class="row-title">
          {#if st.checking}Checking…{:else if health.ready}Ready{:else}Not ready{/if}
        </span>
        <span class="spacer"></span>
        {#if cfg.kind !== "native"}
          <Button variant="ghost" icon="refresh" disabled={st.checking || !!busy} onclick={check}>Check again</Button>
        {/if}
      </div>
      <p class="small">{health.message}</p>
      {#if health.ready && health.python}
        <p class="faint xs">
          Python {health.python} ·
          {health.programs ? "tools can run programs" : "tools can't start programs"}
          {#each health.details as d (d)} · {d}{/each}
        </p>
      {:else if health.details.length}
        <p class="faint xs">{health.details.join(" · ")}</p>
      {/if}
      {#if !health.ready && cfg.kind !== "native"}
        <p class="faint xs">Until the sandbox is ready, agents get no tools.</p>
      {/if}
      {#if cfg.kind === "native" && !health.ready}
        <div class="actions"><Button onclick={gotoTools}>Set up tools</Button></div>
      {/if}
    </div>
  {/if}

  {#if cfg.kind === "container"}
    <div class="card rows">
      <div class="row">
        <div>
          <div class="row-title">Engine</div>
          <div class="row-sub">Chummy uses the one already on this computer. It never installs one.</div>
        </div>
        <select
          value={cfg.container.engine}
          aria-label="Container engine"
          disabled={!!busy}
          onchange={(e) => saveContainer({ engine: e.currentTarget.value as ContainerEngine })}
        >
          <option value="docker">Docker</option>
          <option value="podman">Podman</option>
        </select>
      </div>
      <div class="row">
        <div>
          <div class="row-title">Program</div>
          <div class="row-sub mono" data-selectable>{cfg.container.program || `Found automatically (${engineLabel.toLowerCase()})`}</div>
        </div>
        <div class="btns">
          <Button variant="ghost" disabled={!!busy} onclick={() => program("container")}>Choose…</Button>
          {#if cfg.container.program}
            <Button variant="ghost" disabled={!!busy} onclick={() => autoProgram("container")}>Find it</Button>
          {/if}
        </div>
      </div>
      <div class="row col">
        <div>
          <div class="row-title">Image</div>
          <div class="row-sub">
            Tools run inside this image. It needs <span class="mono">python3</span> (3.14 or newer for the built-in tools)
            and whatever programs you want tools to use. Chummy runs exactly the image you approve: if the name later
            points to a different image, you'll be asked again.
          </div>
        </div>
        <form
          class="inline"
          onsubmit={(e) => {
            e.preventDefault();
            saveContainer({ image });
          }}
        >
          <input class="mono" bind:value={image} aria-label="Image" placeholder={st.default_image} spellcheck="false" />
          <Button type="submit" disabled={!!busy || image.trim() === cfg.container.image}>Save</Button>
        </form>
        <div class="image-state">
          {#if cfg.container.approved_id}
            <span class="ok-text"><Icon name="check" size={13} /> Approved: <span class="mono">{short(cfg.container.approved_id)}</span></span>
          {:else}
            <span class="faint">Not got yet.</span>
          {/if}
          <span class="spacer"></span>
          <Button variant={cfg.container.approved_id ? "ghost" : "primary"} icon="download" disabled={!!busy} onclick={() => getImage(true)}>
            {busy === "pull" ? "Downloading…" : cfg.container.approved_id ? "Download again" : "Download image"}
          </Button>
          <Button variant="ghost" disabled={!!busy} onclick={() => getImage(false)}>Use the copy on this computer</Button>
        </div>
      </div>
      <div class="row">
        <div>
          <div class="row-title">Limits per call</div>
          <div class="row-sub">Memory and processor time each tool call may use.</div>
        </div>
        <div class="btns">
          <select
            value={String(cfg.container.memory_mb)}
            aria-label="Memory per call"
            disabled={!!busy}
            onchange={(e) => saveContainer({ memory_mb: Number(e.currentTarget.value) })}
          >
            {#each [512, 1024, 2048, 4096, 8192] as m (m)}<option value={String(m)}>{m >= 1024 ? `${m / 1024} GB` : `${m} MB`}</option>{/each}
          </select>
          <select
            value={String(cfg.container.cpus)}
            aria-label="CPUs per call"
            disabled={!!busy}
            onchange={(e) => saveContainer({ cpus: Number(e.currentTarget.value) })}
          >
            {#each [1, 2, 4, 8] as c (c)}<option value={String(c)}>{c} CPU{c === 1 ? "" : "s"}</option>{/each}
          </select>
        </div>
      </div>
    </div>
    <p class="faint xs note">
      Each call: no network, read-only system files, only the workspace mounted, all Linux capabilities dropped, no
      privilege escalation, and the container is removed afterwards.
    </p>
  {:else if cfg.kind === "openshell"}
    <div class="card rows">
      <div class="row">
        <div>
          <div class="row-title">Program</div>
          <div class="row-sub mono" data-selectable>{cfg.openshell.program || "Found automatically (openshell)"}</div>
        </div>
        <div class="btns">
          <Button variant="ghost" disabled={!!busy} onclick={() => program("openshell")}>Choose…</Button>
          {#if cfg.openshell.program}
            <Button variant="ghost" disabled={!!busy} onclick={() => autoProgram("openshell")}>Find it</Button>
          {/if}
        </div>
      </div>
      <div class="row col">
        <div>
          <div class="row-title">Sandbox image</div>
          <div class="row-sub">Leave empty for OpenShell's default. It needs <span class="mono">python3</span> (3.14 or newer for the built-in tools).</div>
        </div>
        <form
          class="inline"
          onsubmit={(e) => {
            e.preventDefault();
            saveOsImage();
          }}
        >
          <input class="mono" bind:value={osImage} aria-label="Sandbox image" placeholder="OpenShell's default" spellcheck="false" />
          <Button type="submit" disabled={!!busy || osImage.trim() === cfg.openshell.image}>Save</Button>
        </form>
      </div>
      <div class="row">
        <div>
          <div class="row-title">Chummy's sandboxes</div>
          <div class="row-sub">
            One per agent workspace, kept between calls because making one takes a while.
            {#if removed !== null}Removed {removed}.{/if}
          </div>
        </div>
        <Button variant="ghost" disabled={!!busy} onclick={removeAll}>Remove them</Button>
      </div>
    </div>
    <p class="faint xs note">
      Chummy's policy: Landlock required, an unprivileged user, only system folders readable, and no network rules at
      all. OpenShell can't open a folder directly, so the workspace is copied in for each call and the changes copied
      back. If you change a file while a tool runs, your version is kept. Links never travel either way.
    </p>
  {/if}
{/if}

<p class="faint xs" style="margin-top: var(--space-4)">
  Model connections in Models are made by Chummy itself and work regardless of these switches.
</p>

<style>
  .intro {
    margin: 0 0 var(--space-3);
  }
  .kinds {
    display: grid;
    gap: var(--space-2);
    margin-bottom: var(--space-3);
  }
  .kind {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .kind:hover {
    border-color: var(--border-strong, var(--border));
  }
  .kind.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .dot {
    flex: none;
    width: 14px;
    height: 14px;
    margin-top: 3px;
    border-radius: 50%;
    border: 2px solid var(--text-faint);
  }
  .kind.on .dot {
    border-color: var(--accent);
    background: radial-gradient(var(--accent) 45%, transparent 50%);
  }
  .k-text {
    display: grid;
    gap: 2px;
  }
  .row-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .tag {
    padding: 0 6px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    font-weight: 500;
  }
  .status {
    margin-bottom: var(--space-3);
    display: grid;
    gap: 4px;
  }
  .status.bad {
    border-color: color-mix(in srgb, var(--warn) 45%, var(--border));
  }
  .status-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .status:not(.bad) .status-head :global(svg) {
    color: var(--ok);
  }
  .status.bad .status-head :global(svg) {
    color: var(--warn);
  }
  .spacer {
    flex: 1;
  }
  .btns {
    display: flex;
    gap: var(--space-1);
    flex: none;
  }
  .row.col {
    flex-direction: column;
    align-items: stretch;
    gap: var(--space-2);
  }
  .inline {
    display: flex;
    gap: var(--space-2);
  }
  .inline input {
    flex: 1;
    min-width: 0;
  }
  .image-state {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    font-size: var(--text-sm);
  }
  .ok-text {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--ok);
  }
  .note {
    margin-top: var(--space-2);
  }
  .mono {
    word-break: break-all;
  }
</style>
