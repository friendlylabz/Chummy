<script lang="ts">
  import Icon from "./Icon.svelte";

  let { status }: { status: "pass" | "warn" | "fail" | "skipped" } = $props();
  const icon = $derived(
    status === "pass" ? "check-circle" : status === "warn" ? "alert" : status === "fail" ? "x-circle" : "minus",
  );
</script>

<span class="s {status}" aria-label={status}><Icon name={icon} size={16} /></span>

<style>
  .s {
    display: inline-grid;
    place-items: center;
    flex: none;
  }
  .pass {
    color: var(--ok);
  }
  .warn {
    color: var(--warn);
  }
  .fail {
    color: var(--danger);
  }
  .skipped {
    color: var(--text-faint);
  }
</style>
