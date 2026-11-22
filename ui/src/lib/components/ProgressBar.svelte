<script lang="ts">
  /** `value` from 0 to 1, or null for an indeterminate bar. */
  let { value, label }: { value: number | null; label: string } = $props();
</script>

<div
  class="track"
  role="progressbar"
  aria-label={label}
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={value == null ? undefined : Math.round(value * 100)}
>
  <div
    class="fill"
    class:indeterminate={value == null}
    style:width={value == null ? "30%" : `${Math.max(1, Math.min(100, value * 100))}%`}
  ></div>
</div>

<style>
  .track {
    position: relative;
    height: 6px;
    border-radius: var(--radius-full);
    background: var(--surface-2);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 300ms var(--ease);
  }
  .indeterminate {
    position: absolute;
    animation: slide 1.4s var(--ease) infinite;
  }
  @keyframes slide {
    from {
      left: -30%;
    }
    to {
      left: 100%;
    }
  }
</style>
