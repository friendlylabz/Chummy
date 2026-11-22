<script lang="ts">
  import { commands } from "$lib/ipc";
  import SideNav from "$lib/components/SideNav.svelte";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import Button from "$lib/components/Button.svelte";

  type View = "jobs" | "dispatch";
  const views: { id: View; label: string; icon: "clock" | "play" }[] = [
    { id: "jobs", label: "Jobs", icon: "clock" },
    { id: "dispatch", label: "Dispatch", icon: "play" },
  ];
  let active = $state<View>("jobs");
</script>

<div class="layout">
  <aside class="side">
    <div class="side-title">Scheduler</div>
    <SideNav items={views} bind:active label="Scheduler views" />
  </aside>

  <main class="content">
    {#if active === "jobs"}
      <header class="head">
        <div>
          <h1>Jobs</h1>
          <p class="sub">An agent loop, an input and a schedule.</p>
        </div>
        <Button variant="primary" icon="plus" disabled>New job</Button>
      </header>
      <div class="panel">
        <EmptyState icon="clock" title="No jobs yet">
          Jobs run one of your loops on a schedule, fed from a file, a folder, a web page or
          text you type.
          {#snippet actions()}
            <Button icon="flow" onclick={() => commands.openWindow("editor")}>Open loop editor</Button>
          {/snippet}
        </EmptyState>
      </div>
    {:else}
      <header class="head">
        <div>
          <h1>Dispatch</h1>
          <p class="sub">Start jobs by hand and watch them run.</p>
        </div>
      </header>
      <div class="panel">
        <EmptyState icon="play" title="Nothing has run yet">
          Job runs and their logs appear here.
        </EmptyState>
      </div>
    {/if}
  </main>
</div>

<style>
  .layout {
    display: flex;
    height: 100%;
  }
  .side {
    flex: none;
    width: 200px;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }
  .side-title {
    padding: var(--space-5) var(--space-5) 0;
    font-weight: 650;
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: var(--space-6);
  }
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
    margin-bottom: var(--space-5);
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .sub {
    margin-top: var(--space-1);
    color: var(--text-muted);
  }
  .panel {
    display: flex;
    min-height: 260px;
    border-radius: var(--radius-lg);
    border: 1px dashed var(--border-strong);
  }
</style>
