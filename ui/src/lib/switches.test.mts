// Tests for block switches. Run: npm run test:ui
import assert from "node:assert/strict";
import { describeTools, isDisabled, readSwitch, readTools, setDisabled, switchFor, writeTools } from "./switches.ts";

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

test("tools are disabled one by one", () => {
  let t = readTools(undefined);
  t = setDisabled(t, "run_shell", true);
  assert.deepEqual(writeTools(t), { all: "inherit", each: { run_shell: "off" } });
  assert.ok(isDisabled(readTools(writeTools(t)), "run_shell"));
  assert.ok(!isDisabled(readTools(writeTools(t)), "calc"));
  assert.equal(describeTools(writeTools(t)), "Agent's tools, 1 disabled");
  // Re-enabling stores nothing (it's the same as "all").
  assert.equal(writeTools(setDisabled(t, "run_shell", false)), undefined);
});

test("all disabled, with exceptions", () => {
  const t = setDisabled({ all: "off", each: {} }, "calc", false);
  assert.deepEqual(writeTools(t), { all: "off", each: { calc: "inherit" } });
  assert.ok(!isDisabled(t, "calc") && isDisabled(t, "other"));
  assert.equal(describeTools("off"), "No tools");
  assert.equal(describeTools(writeTools(t)), "Tools off but 1");
});

test("blocks can't turn things on: old \"on\" follows the agent", () => {
  assert.equal(readSwitch("on"), "inherit");
  assert.deepEqual(readTools("on"), { all: "inherit", each: {} });
  assert.deepEqual(readTools({ all: "off", each: { calc: "on" } }).each, { calc: "inherit" });
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
