// Tests for the text box undo history. Run: npm run test:ui
import assert from "node:assert/strict";
import { GROUP_MS, TextUndo, undoKey } from "./undo.ts";

let n = 0;
function test(name: string, f: () => void) {
  f();
  n++;
  console.log(`ok - ${name}`);
}
const at = (value: string) => ({ value, start: value.length, end: value.length });

/** Types `text` one key at a time from `from`, 50 ms apart. */
function type(u: TextUndo, from: string, text: string, t0: number): [string, number] {
  let v = from;
  let t = t0;
  for (const ch of text) {
    u.before(at(v), "insertText", ch, t);
    v += ch;
    t += 50;
  }
  return [v, t];
}

test("a word typed quickly is one undo step; a space starts the next", () => {
  const u = new TextUndo();
  let [v, t] = type(u, "", "hello", 0);
  [v, t] = type(u, v, " world", t);
  assert.equal(v, "hello world");
  const a = u.undo(at(v))!;
  assert.equal(a.value, "hello");
  const b = u.undo(a)!;
  assert.equal(b.value, "");
  assert.equal(u.undo(b), null);
});

test("a pause starts a new step", () => {
  const u = new TextUndo();
  let [v, t] = type(u, "", "ab", 0);
  [v] = type(u, v, "cd", t + GROUP_MS + 1);
  assert.equal(u.undo(at(v))!.value, "ab");
});

test("redo goes forward again, and a new change forgets it", () => {
  const u = new TextUndo();
  const [v] = type(u, "", "hi", 0);
  const back = u.undo(at(v))!;
  assert.equal(back.value, "");
  const fwd = u.redo(back)!;
  assert.equal(fwd.value, "hi");
  u.undo(fwd);
  u.before(at(""), "insertText", "x", 10_000);
  assert.equal(u.redo(at("x")), null);
});

test("a paste or deletion is its own step", () => {
  const u = new TextUndo();
  let [v, t] = type(u, "", "abc", 0);
  u.before(at(v), "deleteContentBackward", null, t);
  v = "ab";
  u.before(at(v), "insertFromPaste", "XYZ", t + 10);
  v = "abXYZ";
  assert.equal(u.undo(at(v))!.value, "ab");
  assert.equal(u.undo(at("ab"))!.value, "abc");
});

test("clear forgets the history", () => {
  const u = new TextUndo();
  type(u, "", "sent", 0);
  u.clear();
  assert.equal(u.undo(at("")), null);
});

test("keys: Ctrl/Cmd+Z undoes; with Shift, or Ctrl+Y, redoes", () => {
  const k = (key: string, o: Partial<KeyboardEvent> = {}) =>
    undoKey({ key, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...o });
  assert.equal(k("z", { ctrlKey: true }), "undo");
  assert.equal(k("z", { metaKey: true }), "undo");
  assert.equal(k("Z", { ctrlKey: true, shiftKey: true }), "redo");
  assert.equal(k("y", { ctrlKey: true }), "redo");
  assert.equal(k("z"), null);
  assert.equal(k("z", { ctrlKey: true, altKey: true }), null);
});

console.log(`${n} passed`);
