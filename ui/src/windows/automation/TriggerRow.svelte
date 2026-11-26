<script lang="ts">
  import type { Day, Trigger } from "$lib/ipc";
  import Button from "$lib/components/Button.svelte";

  let { trigger = $bindable(), onremove }: { trigger: Trigger; onremove: () => void } = $props();

  const days: { id: Day; label: string }[] = [
    { id: "mon", label: "M" },
    { id: "tue", label: "T" },
    { id: "wed", label: "W" },
    { id: "thu", label: "T" },
    { id: "fri", label: "F" },
    { id: "sat", label: "S" },
    { id: "sun", label: "S" },
  ];
  const dayNames: Record<Day, string> = {
    mon: "Monday",
    tue: "Tuesday",
    wed: "Wednesday",
    thu: "Thursday",
    fri: "Friday",
    sat: "Saturday",
    sun: "Sunday",
  };

  type Unit = "minutes" | "hours" | "days";
  const mult: Record<Unit, number> = { minutes: 1, hours: 60, days: 1440 };
  function split(m: number): [number, Unit] {
    if (m % 1440 === 0) return [m / 1440, "days"];
    if (m % 60 === 0) return [m / 60, "hours"];
    return [m, "minutes"];
  }
  let unit = $state<Unit>(trigger.kind === "every" ? split(trigger.minutes)[1] : "hours");
  let amount = $state(trigger.kind === "every" ? split(trigger.minutes)[0] : 1);

  function setEvery(a: number, u: Unit) {
    amount = a;
    unit = u;
    if (trigger.kind === "every") trigger.minutes = Math.max(1, Math.round(a)) * mult[u];
  }

  function toggleDay(d: Day) {
    if (trigger.kind !== "daily") return;
    trigger.days = trigger.days.includes(d) ? trigger.days.filter((x) => x !== d) : [...trigger.days, d];
  }
</script>

<div class="trigger">
  {#if trigger.kind === "daily"}
    <span class="what">At</span>
    <input class="input narrow" type="time" value={trigger.time} oninput={(e) => trigger.kind === "daily" && (trigger.time = e.currentTarget.value)} aria-label="Time" />
    <span class="what">on</span>
    <div class="days" role="group" aria-label="Days">
      {#each days as d (d.id)}
        <button
          type="button"
          class="day"
          class:on={trigger.days.includes(d.id)}
          aria-pressed={trigger.days.includes(d.id)}
          aria-label={dayNames[d.id]}
          title={dayNames[d.id]}
          onclick={() => toggleDay(d.id)}>{d.label}</button
        >
      {/each}
    </div>
    {#if !trigger.days.length}<span class="faint">every day</span>{/if}
  {:else if trigger.kind === "every"}
    <span class="what">Every</span>
    <input
      class="input tiny"
      type="number"
      min="1"
      value={amount}
      aria-label="How often"
      oninput={(e) => setEvery(Number(e.currentTarget.value) || 1, unit)}
    />
    <select class="input narrow" value={unit} aria-label="Unit" onchange={(e) => setEvery(amount, e.currentTarget.value as Unit)}>
      <option value="minutes">minutes</option>
      <option value="hours">hours</option>
      <option value="days">days</option>
    </select>
    <span class="faint">
      {#if trigger.minutes < 1440}About {Math.floor(1440 / trigger.minutes)} runs a day{trigger.minutes < 60 ? "; with a paid model, that adds up" : ""}.{:else}From when it's switched on.{/if}
    </span>
  {:else if trigger.kind === "once"}
    <span class="what">Once, on</span>
    <input
      class="input"
      type="datetime-local"
      value={trigger.at}
      oninput={(e) => trigger.kind === "once" && (trigger.at = e.currentTarget.value)}
      aria-label="Date and time"
    />
  {:else}
    <span class="what">When files change in</span>
    <input
      class="input mono"
      value={trigger.path}
      oninput={(e) => trigger.kind === "folder" && (trigger.path = e.currentTarget.value)}
      placeholder="inbox"
      aria-label="Folder in the agent's folder"
    />
  {/if}
  <Button variant="ghost" icon="close" iconOnly aria-label="Remove this schedule" onclick={onremove} />
</div>

<style>
  .trigger {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--surface-2);
  }
  .what {
    color: var(--text-muted);
    font-size: var(--text-sm);
  }
  .faint {
    color: var(--text-faint);
    font-size: var(--text-xs);
    flex: 1;
  }
  .input {
    height: 32px;
    padding: 0 var(--space-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    background: var(--bg);
    min-width: 0;
    flex: 1;
  }
  .input.narrow {
    flex: none;
    width: 110px;
  }
  .input.tiny {
    flex: none;
    width: 70px;
  }
  .input.mono {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
  }
  .days {
    display: flex;
    gap: 3px;
  }
  .day {
    width: 28px;
    height: 28px;
    border: 1px solid var(--border-strong);
    border-radius: 50%;
    background: transparent;
    font-size: var(--text-xs);
    font-weight: 600;
    cursor: pointer;
  }
  .day.on {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
</style>
