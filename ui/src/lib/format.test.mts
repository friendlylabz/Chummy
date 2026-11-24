// Tests for display helpers. Run: npm run test:ui
import assert from "node:assert/strict";
import { stopwatch } from "./format.ts";

let n = 0;
function test(name: string, f: () => void) {
  f();
  n++;
  console.log(`ok - ${name}`);
}

test("stopwatch shows tenths of a second", () => {
  assert.equal(stopwatch(0), "0.0s");
  assert.equal(stopwatch(99), "0.0s");
  assert.equal(stopwatch(4250), "4.2s");
  assert.equal(stopwatch(59_999), "59.9s");
});

test("stopwatch adds minutes and hours", () => {
  assert.equal(stopwatch(60_000), "1:00.0");
  assert.equal(stopwatch(65_300), "1:05.3");
  assert.equal(stopwatch(12 * 60_000 + 7_800), "12:07.8");
  assert.equal(stopwatch(3_600_000 + 2 * 60_000 + 5_100), "1:02:05.1");
});

test("stopwatch never goes negative", () => {
  assert.equal(stopwatch(-500), "0.0s");
});

console.log(`${n} passed`);
