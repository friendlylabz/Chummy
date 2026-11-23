// Tests for block switches. Run: npm run test:ui
import assert from "node:assert/strict";
import { describeTools, readSwitch, readTools, switchFor, writeTools } from "./switches.ts";

let n = 0;
function test(name: string, f: () => void) {
  f();
  n++;
  console.log(`ok - ${name}`);
}

test("left out means the agent's tools", () => {
  assert.deepEqual(readTools(undefined), { all: "inherit", each: {} });
  assert.equal(writeTools({ all: "inherit", each: {} }), undefined);
  assert.equal(describeTools(undefined), "Agent's tools");
});

test("per-tool switches round-trip", () => {
  const t = { all: "off" as const, each: { calc: "on" as const, fetch: "off" as const } };
  // A tool set the same as "all" isn't stored.
  assert.deepEqual(writeTools(t), { all: "off", each: { calc: "on" } });
  assert.equal(switchFor(readTools(writeTools(t)), "calc"), "on");
  assert.equal(switchFor(readTools(writeTools(t)), "fetch"), "off");
  assert.equal(describeTools(writeTools(t)), "1 tool");
});

test("old forms still read", () => {
  const t = readTools(["calc"]);
  assert.equal(switchFor(t, "calc"), "inherit");
  assert.equal(switchFor(t, "other"), "off");
  assert.equal(readSwitch(true), "inherit");
  assert.equal(readSwitch(false), "off");
  assert.equal(readSwitch(undefined), "inherit");
});

console.log(`${n} passed`);
