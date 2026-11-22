<script lang="ts">
  import { commands, type Progress } from "$lib/ipc";
  import { bytes, duration } from "$lib/format";
  import Button from "$lib/components/Button.svelte";
  import ProgressBar from "$lib/components/ProgressBar.svelte";

  let { repo, progress }: { repo: string; progress: Progress | null } = $props();

  const frac = $derived(progress && progress.total_bytes ? progress.done_bytes / progress.total_bytes : null);
  const eta = $derived(
    progress && progress.bytes_per_sec > 0 ? (progress.total_bytes - progress.done_bytes) / progress.bytes_per_sec : 0,
  );
</script>

<div class="dl">
  <div class="top">
    <span class="what">
      {#if progress}
        {bytes(progress.done_bytes)} of {bytes(progress.total_bytes)}
        {#if progress.bytes_per_sec > 0}· {bytes(progress.bytes_per_sec)}/s{/if}
        {#if eta}· {duration(eta)} left{/if}
      {:else}
        Starting…
      {/if}
    </span>
    <Button variant="ghost" onclick={() => commands.cancelDownload(repo)}>Cancel</Button>
  </div>
  <ProgressBar value={frac} label={`Downloading ${repo}`} />
  {#if progress && progress.file_index < progress.file_count}
    <div class="file">File {progress.file_index + 1} of {progress.file_count}: {progress.file}</div>
  {/if}
</div>

<style>
  .dl {
    display: grid;
    gap: var(--space-2);
  }
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .what {
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
  }
  .file {
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-family: var(--font-mono);
  }
</style>
