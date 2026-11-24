// Tests for generation settings helpers. Run: npm run test:ui
import assert from "node:assert/strict";
import { describe, PARAMS, parseParam, showParam, unsupported } from "./generation.ts";

let n = 0;
function test(name: string, f: () => void) {
  f();
  n++;
  console.log(`ok - ${name}`);
}
const info = (key: string) => PARAMS.find((p) => p.key === key)!;

test("numbers are checked against their range", () => {
  assert.deepEqual(parseParam(info("top_p"), " 0.9 "), { value: 0.9 });
  assert.deepEqual(parseParam(info("top_p"), ""), { value: null });
  assert.ok("error" in parseParam(info("top_p"), "1.5"));
  assert.ok("error" in parseParam(info("top_p"), "abc"));
  assert.ok("error" in parseParam(info("top_k"), "2.5"));
  assert.deepEqual(parseParam(info("presence_penalty"), "-1"), { value: -1 });
});

test("stop sequences are one per line, at most four", () => {
  assert.deepEqual(parseParam(info("stop"), "END\n\n###\n"), { value: ["END", "###"] });
  assert.ok("error" in parseParam(info("stop"), "a\nb\nc\nd\ne"));
  assert.equal(showParam("stop", ["a", "b"]), "a\nb");
});

test("support mirrors the Rust table", () => {
  for (const p of PARAMS) {
    assert.equal(unsupported("open_ai_compatible", p.key), null);
    assert.equal(unsupported("ollama", p.key), null);
    assert.ok(unsupported("local", p.key));
  }
  assert.ok(unsupported("open_ai", "min_p"));
  assert.equal(unsupported("open_ai", "reasoning_effort"), null);
  assert.ok(unsupported("anthropic", "reasoning_effort"));
  assert.equal(unsupported("anthropic", "top_k"), null);
  assert.ok(unsupported("anthropic", "seed"));
  assert.equal(unsupported("gemini", "reasoning_effort"), null);
  assert.ok(unsupported("gemini", "repeat_penalty"));
});

test("values are described in words", () => {
  assert.equal(describe("float", null), "not sent");
  assert.equal(describe("stop", []), "not sent");
  assert.equal(describe("effort", "low"), "low");
  assert.equal(describe("float", 0.7), "0.7");
});

console.log(`${n} passed`);
