<script lang="ts">
  /**
   * A model's text, formatted (Markdown and math) as Settings → Appearance
   * says. Links ask before they open; code blocks have a Copy button.
   */
  import { commands } from "$lib/ipc";
  import { format, hasMath, loadMath, mathLoaded } from "$lib/markdown";
  import { settings } from "$lib/stores/settings.svelte";

  let { text }: { text: string } = $props();

  const opts = $derived({
    markdown: settings.current.general.markdown ?? true,
    math: settings.current.general.math ?? true,
  });
  // Re-render once KaTeX has loaded, if this text needs it.
  let mathReady = $state(mathLoaded());
  $effect(() => {
    if (opts.math && !mathReady && hasMath(text)) loadMath().then(() => (mathReady = mathLoaded()));
  });
  const html = $derived.by(() => {
    void mathReady;
    return format(text, opts);
  });

  function onclick(e: MouseEvent) {
    const t = e.target as HTMLElement;
    const a = t.closest<HTMLAnchorElement>("a.md-link");
    if (a) {
      e.preventDefault();
      const url = a.dataset.href;
      if (url) commands.openLink(url);
      return;
    }
    const copy = t.closest<HTMLButtonElement>("button[data-copy]");
    if (copy) {
      const code = copy.closest(".code-block")?.querySelector("code")?.textContent ?? "";
      navigator.clipboard?.writeText(code).then(() => {
        copy.textContent = "Copied";
        setTimeout(() => (copy.textContent = "Copy"), 1200);
      });
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="md" class:plain={!opts.markdown} data-selectable {onclick}>{@html html}</div>

<style>
  .md {
    line-height: 1.6;
    word-break: break-word;
  }
  .md :global(p) {
    margin: 0 0 0.6em;
  }
  .md :global(p:last-child),
  .md :global(:is(ul, ol, blockquote, .code-block, .table-wrap, .math-block):last-child) {
    margin-bottom: 0;
  }
  .md :global(p.plain) {
    white-space: pre-wrap;
  }
  .md :global(.md-h) {
    margin: 0.9em 0 0.35em;
    font-weight: 650;
    line-height: 1.3;
    letter-spacing: -0.01em;
  }
  .md :global(.md-h:first-child) {
    margin-top: 0;
  }
  .md :global(h1.md-h) {
    font-size: 1.3em;
  }
  .md :global(h2.md-h) {
    font-size: 1.18em;
  }
  .md :global(h3.md-h) {
    font-size: 1.06em;
  }
  .md :global(:is(h4, h5, h6).md-h) {
    font-size: 1em;
    color: var(--text-muted);
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0 0 0.6em;
    padding-left: 1.4em;
  }
  .md :global(li) {
    margin: 0.15em 0;
  }
  .md :global(li > ul),
  .md :global(li > ol) {
    margin: 0.15em 0 0;
  }
  .md :global(.task) {
    color: var(--text-muted);
  }
  .md :global(.task.done) {
    color: var(--ok);
  }
  .md :global(blockquote) {
    margin: 0 0 0.6em;
    padding: 0.1em 0 0.1em 0.9em;
    border-left: 3px solid var(--border-strong);
    color: var(--text-muted);
  }
  .md :global(hr) {
    margin: 0.9em 0;
    border: none;
    border-top: 1px solid var(--border);
  }
  .md :global(code) {
    padding: 0.05em 0.35em;
    border-radius: 4px;
    background: var(--surface-2);
    font-family: var(--font-mono);
    font-size: 0.9em;
  }
  .md :global(.code-block) {
    margin: 0 0 0.6em;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface-2);
    overflow: hidden;
  }
  .md :global(.code-head) {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 3px var(--space-3);
    border-bottom: 1px solid var(--border);
    color: var(--text-faint);
    font-size: var(--text-xs);
    font-family: var(--font-mono);
  }
  .md :global(.copy) {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .md :global(.code-block pre) {
    margin: 0;
    padding: var(--space-2) var(--space-3);
    overflow-x: auto;
  }
  .md :global(.code-block code) {
    padding: 0;
    background: none;
    font-size: var(--text-xs);
    line-height: 1.55;
    white-space: pre;
  }
  .md :global(.table-wrap) {
    margin: 0 0 0.6em;
    overflow-x: auto;
  }
  .md :global(table) {
    border-collapse: collapse;
    font-size: 0.94em;
  }
  .md :global(th),
  .md :global(td) {
    padding: 4px 10px;
    border: 1px solid var(--border);
    text-align: left;
  }
  .md :global(th) {
    background: var(--surface-2);
    font-weight: 600;
  }
  .md :global(.md-link) {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .md :global(.math-block) {
    margin: 0 0 0.6em;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .md :global(.math-src) {
    color: var(--text-muted);
  }
  .md :global(.katex) {
    font-size: 1.08em;
  }
</style>
