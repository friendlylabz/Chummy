/**
 * Formatting for model replies: Markdown (headings, lists, quotes, tables,
 * code, emphasis, links) and LaTeX math. Built for untrusted text: every
 * character of the input is escaped before any markup is added, links only
 * become buttons (Rust asks before opening them), and math goes through
 * KaTeX with `trust` off. No raw HTML from the model ever reaches the page.
 */

export interface FormatOptions {
  /** Markdown; off shows the text as it is. */
  markdown: boolean;
  /** LaTeX math ($…$, $$…$$, \(…\), \[…\]). */
  math: boolean;
}

/** The parts of KaTeX used here (loaded only when a reply has math). */
export interface Katex {
  renderToString(tex: string, options: Record<string, unknown>): string;
}

let katex: Katex | null = null;
let loading: Promise<Katex | null> | null = null;

/** Loads KaTeX (and its styles and fonts) once. */
export function loadMath(): Promise<Katex | null> {
  loading ??= Promise.all([import("katex"), import("katex/dist/katex.min.css")])
    .then(([m]) => (katex = (m.default ?? m) as Katex))
    .catch(() => null);
  return loading;
}

/** Uses an already loaded KaTeX (tests). */
export function setMath(k: Katex | null) {
  katex = k;
}

export function mathLoaded(): boolean {
  return katex !== null;
}

/** Whether text looks like it has math worth loading KaTeX for. */
export function hasMath(s: string): boolean {
  return /\$\$|\\\(|\\\[|(^|[^\\$\w])\$[^\s$][^$\n]*?[^\s\\$]\$(?![\w$])|(^|[^\\$\w])\$[^\s$\d]\$/.test(s);
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function renderMath(tex: string, display: boolean): string {
  if (!katex) {
    const d = display ? "$$" : "$";
    return `<code class="math-src">${escapeHtml(d + tex + d)}</code>`;
  }
  try {
    return katex.renderToString(tex, {
      displayMode: display,
      throwOnError: false,
      trust: false,
      strict: "ignore",
      maxSize: 20,
      maxExpand: 500,
      output: "htmlAndMathml",
    });
  } catch {
    return `<code class="math-src">${escapeHtml(tex)}</code>`;
  }
}

// ------------------------------------------------------------- inline

/** Placeholders for pieces already turned into HTML. */
class Stash {
  items: string[] = [];
  put(html: string): string {
    this.items.push(html);
    return `\u0000${this.items.length - 1}\u0000`;
  }
  restore(s: string): string {
    return s.replace(/\u0000(\d+)\u0000/g, (_, i) => this.items[Number(i)] ?? "");
  }
}

const SAFE_URL = /^(https?:\/\/|mailto:)[^\s<>"']+$/i;

function link(url: string, text: string): string {
  const u = url.trim();
  if (!SAFE_URL.test(u)) return text;
  return `<a class="md-link" href="#" data-href="${escapeHtml(u)}" title="${escapeHtml(u)}">${text}</a>`;
}

/**
 * A paragraph line, keeping its leading indentation (HTML would collapse it),
 * so nested text such as a plan outline stays nested. Tabs count as 4.
 */
function indented(line: string, opts: FormatOptions): string {
  const lead = /^[ \t]+/.exec(line);
  if (!lead) return inline(line, opts);
  const n = lead[0].replace(/\t/g, "    ").length;
  return "&nbsp;".repeat(n) + inline(line.slice(lead[0].length), opts);
}

export function inline(raw: string, opts: FormatOptions, stash = new Stash()): string {
  let s = raw.replace(/\u0000/g, "");
  // Code first: nothing inside it is formatted.
  s = s.replace(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g, (_, _t, code: string) =>
    stash.put(`<code>${escapeHtml(code.replace(/^ (.*) $/, "$1"))}</code>`),
  );
  if (opts.math) {
    s = s.replace(/\\\((.+?)\\\)/g, (_, tex: string) => stash.put(renderMath(tex, false)));
    s = s.replace(/\$\$([^$]+?)\$\$/g, (_, tex: string) => stash.put(renderMath(tex, true)));
    // $…$ only when it can't be money: no space just inside the dollars,
    // and no digit right after the closing one.
    s = s.replace(/(^|[^\\$\w])\$(?![\s$])((?:[^$\n\\]|\\.)*?[^\s\\$])\$(?![\w$])/g, (_, pre: string, tex: string) =>
      /^\d[\d.,]*$/.test(tex) ? `${pre}$${tex}$` : pre + stash.put(renderMath(tex, false)),
    );
  }
  // Links: [text](url) and bare URLs.
  s = s.replace(/\[([^\]\n]+)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g, (_, text: string, url: string) =>
    stash.put(link(url, inline(text, { ...opts, math: false }, stash))),
  );
  s = s.replace(/\bhttps?:\/\/[^\s<>()"']+[^\s<>()"'.,;:!?]/g, (url) => stash.put(link(url, escapeHtml(url))));

  s = escapeHtml(s);
  s = s
    .replace(/\*\*\*(?=\S)([\s\S]*?\S)\*\*\*/g, "<strong><em>$1</em></strong>")
    .replace(/\*\*(?=\S)([\s\S]*?\S)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w])__(?=\S)([\s\S]*?\S)__(?!\w)/g, "$1<strong>$2</strong>")
    .replace(/(^|[^*\w])\*(?=[^\s*])([^*\n]*?[^\s*])\*(?![*\w])/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_(?=[^\s_])([^_\n]*?[^\s_])_(?!\w)/g, "$1<em>$2</em>")
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, "<del>$1</del>");
  return stash.restore(s);
}

// -------------------------------------------------------------- blocks

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([\w+#.-]*)[^\n]*$/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^ {0,3}([-*_])(\s*\1){2,}\s*$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;

function cells(line: string): string[] {
  let l = line.trim();
  if (l.startsWith("|")) l = l.slice(1);
  if (l.endsWith("|") && !l.endsWith("\\|")) l = l.slice(0, -1);
  // Split on pipes that aren't escaped (no lookbehind: older WebKit lacks it).
  return l
    .replace(/\\\|/g, "\u0001")
    .split("|")
    .map((c) => c.trim().replace(/\u0001/g, "|"));
}

interface ListItem {
  indent: number;
  ordered: boolean;
  start: number;
  lines: string[];
}

function list(lines: string[], opts: FormatOptions): string {
  // Items, with their continuation lines; nested lists are rendered from
  // each item's own lines.
  const items: ListItem[] = [];
  const base = (lines[0].match(/^\s*/) ?? [""])[0].length;
  for (const line of lines) {
    const m = line.match(ITEM);
    if (m && m[1].length <= base + 1) {
      const marker = m[2];
      items.push({
        indent: m[1].length,
        ordered: /\d/.test(marker),
        start: parseInt(marker, 10) || 1,
        lines: [m[3]],
      });
    } else if (items.length) {
      items[items.length - 1].lines.push(line.replace(new RegExp(`^ {0,${base + 4}}`), ""));
    }
  }
  if (!items.length) return "";
  // A change between - and 1. starts a new list.
  const groups: ListItem[][] = [];
  for (const it of items) {
    const g = groups[groups.length - 1];
    if (g && g[0].ordered === it.ordered) g.push(it);
    else groups.push([it]);
  }
  return groups.map((g) => listHtml(g, opts)).join("");
}

function listHtml(items: ListItem[], opts: FormatOptions): string {
  const ordered = items[0].ordered;
  const start = ordered && items[0].start !== 1 ? ` start="${items[0].start}"` : "";
  const body = items
    .map((it) => {
      const [first, ...rest] = it.lines;
      const task = first.match(/^\[([ xX])\]\s+(.*)$/);
      const head = task
        ? `<span class="task ${task[1] === " " ? "" : "done"}">${task[1] === " " ? "☐" : "☑"}</span> ${inline(task[2], opts)}`
        : inline(first, opts);
      const more = rest.join("\n").trim() ? blocks(rest.join("\n"), opts) : "";
      return `<li>${head}${more}</li>`;
    })
    .join("");
  return ordered ? `<ol${start}>${body}</ol>` : `<ul>${body}</ul>`;
}

function table(rows: string[], opts: FormatOptions): string {
  const head = cells(rows[0]);
  const align = cells(rows[1]).map((c) =>
    c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : "",
  );
  const td = (tag: string, c: string, i: number) =>
    `<${tag}${align[i] ? ` style="text-align:${align[i]}"` : ""}>${inline(c, opts)}</${tag}>`;
  const body = rows
    .slice(2)
    .map((r) => `<tr>${cells(r).slice(0, head.length).map((c, i) => td("td", c, i)).join("")}</tr>`)
    .join("");
  return `<div class="table-wrap"><table><thead><tr>${head.map((c, i) => td("th", c, i)).join("")}</tr></thead><tbody>${body}</tbody></table></div>`;
}

function blocks(text: string, opts: FormatOptions): string {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push(`<p>${para.map((l) => indented(l, opts)).join("<br>")}</p>`);
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    let m: RegExpMatchArray | null;
    if ((m = line.match(FENCE))) {
      flush();
      const fence = m[1];
      const lang = m[2];
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence)) code.push(lines[i++]);
      out.push(
        `<div class="code-block"><div class="code-head"><span>${escapeHtml(lang || "code")}</span>` +
          `<button type="button" class="copy" data-copy>Copy</button></div><pre><code>${escapeHtml(code.join("\n"))}</code></pre></div>`,
      );
      continue;
    }
    if (opts.math && /^\s*(\$\$|\\\[)\s*$/.test(line)) {
      flush();
      const close = line.trim() === "$$" ? "$$" : "\\]";
      const tex: string[] = [];
      i++;
      while (i < lines.length && lines[i].trim() !== close) tex.push(lines[i++]);
      out.push(`<div class="math-block">${renderMath(tex.join("\n"), true)}</div>`);
      continue;
    }
    if (opts.math && (m = line.match(/^\s*(?:\$\$(.+)\$\$|\\\[(.+)\\\])\s*$/))) {
      flush();
      out.push(`<div class="math-block">${renderMath(m[1] ?? m[2], true)}</div>`);
      continue;
    }
    if (!line.trim()) {
      flush();
      continue;
    }
    if ((m = line.match(HEADING))) {
      flush();
      const n = m[1].length;
      out.push(`<h${n} class="md-h">${inline(m[2], opts)}</h${n}>`);
      continue;
    }
    if (RULE.test(line) && !para.length) {
      flush();
      out.push("<hr>");
      continue;
    }
    if (QUOTE.test(line)) {
      flush();
      const q: string[] = [];
      while (i < lines.length && QUOTE.test(lines[i])) q.push(lines[i++].match(QUOTE)![1]);
      i--;
      out.push(`<blockquote>${blocks(q.join("\n"), opts)}</blockquote>`);
      continue;
    }
    if (ITEM.test(line) && !RULE.test(line)) {
      flush();
      const l: string[] = [];
      while (i < lines.length && (ITEM.test(lines[i]) || (lines[i].trim() && /^\s+/.test(lines[i])) || (!lines[i].trim() && i + 1 < lines.length && (ITEM.test(lines[i + 1]) || /^\s{2,}\S/.test(lines[i + 1]))))) {
        l.push(lines[i++]);
      }
      i--;
      out.push(list(l.filter((x) => x.trim()), opts));
      continue;
    }
    if (line.includes("|") && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1])) {
      flush();
      const rows = [line, lines[i + 1]];
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(lines[i++]);
      i--;
      out.push(table(rows, opts));
      continue;
    }
    para.push(line);
  }
  flush();
  return out.join("");
}

/** Formats a model's reply as safe HTML. */
export function format(text: string, opts: FormatOptions): string {
  if (!opts.markdown) {
    if (!opts.math) return `<p class="plain">${escapeHtml(text)}</p>`;
    // Math alone: keep the text as it is, with math rendered.
    const stash = new Stash();
    const t = text.replace(/\u0000/g, "");
    const withMath = t
      .replace(/\$\$([\s\S]+?)\$\$|\\\[([\s\S]+?)\\\]/g, (_, a, b) => stash.put(renderMath(a ?? b, true)))
      .replace(/\\\((.+?)\\\)/g, (_, tex) => stash.put(renderMath(tex, false)));
    return `<p class="plain">${stash.restore(escapeHtml(withMath))}</p>`;
  }
  return blocks(text, opts);
}
