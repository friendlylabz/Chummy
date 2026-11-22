// Tests for the reply formatter. Run: npm run test:ui
// (Node's type stripping; no test framework needed.)
import assert from "node:assert/strict";
import katex from "katex";
import { format, hasMath, setMath } from "./markdown.ts";

const on = { markdown: true, math: true };
let n = 0;
function test(name: string, f: () => void) {
  f();
  n++;
  console.log(`ok - ${name}`);
}

test("model text can't add HTML", () => {
  const h = format('<script>alert(1)</script><img src=x onerror="y"> **b**', on);
  assert(!h.includes("<script") && !h.includes("<img"));
  assert(h.includes("&lt;script&gt;") && h.includes("<strong>b</strong>"));
});

test("only web and mail links, as buttons", () => {
  const h = format("[a](javascript:alert(1)) [b](https://x.org/p?q=1) https://y.org/z.", on);
  assert(!h.includes('data-href="javascript'));
  assert(h.includes('data-href="https://x.org/p?q=1"'));
  assert(h.includes('data-href="https://y.org/z"'), h);
  assert(!/ href="http/.test(h), "real hrefs are never set");
});

test("headings, lists, quotes, tables, code", () => {
  const h = format("# T\n- a\n  - b\n1. c\n> q\n\n| x | y |\n|---|:-:|\n| 1 | 2 |\n\n```js\nlet a = 1 < 2;\n```", on);
  for (const s of ["<h1", "<ul><li>a<ul><li>b</li></ul></li></ul><ol><li>c</li></ol>", "<blockquote>", "<table>", "text-align:center", "let a = 1 &lt; 2;", "data-copy"]) {
    assert(h.includes(s), `${s} in ${h}`);
  }
});

test("money isn't math", () => {
  assert.equal(hasMath("It costs $5 and $10."), false);
  assert.equal(hasMath("so $x+1$ is"), true);
  setMath(katex);
  const h = format("Costs $5 to $10; area $\\pi r^2$.", on);
  assert(h.includes("$5 to $10"));
  assert((h.match(/class="katex"/g) ?? []).length === 1);
});

test("math can't link out", () => {
  setMath(katex);
  const h = format("$\\href{javascript:alert(1)}{x}$", on);
  assert(!/href="javascript/.test(h));
});

test("off means plain text", () => {
  const h = format("# **x**", { markdown: false, math: false });
  assert.equal(h, '<p class="plain"># **x**</p>');
});

console.log(`${n} passed`);
