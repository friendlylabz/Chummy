<script lang="ts">
  /** A note on the canvas: text for people, ignored when the loop runs. */
  import { NodeResizer, type NodeProps } from "@xyflow/svelte";
  import { editorActions, type NoteNode } from "./loopdoc";

  let { id, data, selected }: NodeProps<NoteNode> = $props();
  let editing = $state(false);
  let area = $state<HTMLTextAreaElement>();

  function start() {
    editing = true;
    queueMicrotask(() => area?.focus());
  }
</script>

<NodeResizer minWidth={140} minHeight={60} isVisible={selected} lineClass="note-line" handleClass="note-handle" />
<div class="note" class:selected role="note" ondblclick={start}>
  {#if editing}
    <textarea
      bind:this={area}
      class="nodrag nowheel"
      value={data.note.text}
      spellcheck="true"
      aria-label="Note"
      oninput={(e) => editorActions.setNote(id, (e.currentTarget as HTMLTextAreaElement).value)}
      onblur={() => (editing = false)}
      onkeydown={(e) => {
        if (e.key === "Escape") (e.currentTarget as HTMLTextAreaElement).blur();
        e.stopPropagation();
      }}
    ></textarea>
  {:else}
    <div class="text" class:empty={!data.note.text.trim()}>
      {data.note.text.trim() || "Double-click to write a note"}
    </div>
  {/if}
</div>

<style>
  .note {
    width: 100%;
    height: 100%;
    padding: var(--space-2) var(--space-3);
    border: 1px solid color-mix(in srgb, #e8c16d 45%, var(--border));
    border-radius: var(--radius-md);
    background: color-mix(in srgb, #e8c16d 13%, var(--surface));
    color: var(--text);
    font-size: var(--text-sm);
    line-height: 1.5;
    overflow: hidden;
  }
  .note.selected {
    border-color: #e8c16d;
  }
  .text {
    height: 100%;
    overflow: hidden;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .text.empty {
    color: var(--text-faint);
    font-style: italic;
  }
  textarea {
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    color: inherit;
    font: inherit;
    line-height: inherit;
  }
  :global(.note-line) {
    border-color: #e8c16d !important;
  }
  :global(.note-handle) {
    width: 8px !important;
    height: 8px !important;
    border-radius: 2px !important;
    background: #e8c16d !important;
    border: none !important;
  }
</style>
