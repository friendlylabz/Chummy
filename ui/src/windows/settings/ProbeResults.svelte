<script lang="ts">
  import type { ProbeReport } from "$lib/ipc";
  import StatusIcon from "$lib/components/StatusIcon.svelte";

  let { report }: { report: ProbeReport } = $props();
</script>

<ul class="steps">
  {#each report.steps as step (step.name)}
    <li>
      <StatusIcon status={step.status} />
      <div class="text">
        <div class="name">
          {step.name}
          {#if step.status !== "skipped" && step.ms > 0}<span class="ms">{(step.ms / 1000).toFixed(1)} s</span>{/if}
        </div>
        {#if step.detail}<div class="detail" data-selectable>{step.detail}</div>{/if}
      </div>
    </li>
  {/each}
</ul>

<style>
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-2);
  }
  li {
    display: flex;
    gap: var(--space-2);
    align-items: flex-start;
  }
  li :global(.s) {
    margin-top: 2px;
  }
  .name {
    font-size: var(--text-sm);
    font-weight: 550;
  }
  .ms {
    margin-left: var(--space-2);
    color: var(--text-faint);
    font-weight: 400;
    font-size: var(--text-xs);
  }
  .detail {
    color: var(--text-muted);
    font-size: var(--text-sm);
    word-break: break-word;
  }
</style>
