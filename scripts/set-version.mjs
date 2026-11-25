#!/usr/bin/env node
// Sets Chummy's version everywhere it's written down, from a tag or version:
//
//   node scripts/set-version.mjs v0.1.3     (or 0.1.3)
//   node scripts/set-version.mjs --check v0.1.3   (only says whether they match)
//
// The release workflow runs it with the release's tag before building, so the
// app and its installers always carry the tag's version. Run it yourself
// after a release to bring the repository up to date.
//
// Files: Cargo.toml ([workspace.package]), Cargo.lock (Chummy's own crates),
// src-tauri/tauri.conf.json, package.json and package-lock.json.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const check = args.includes("--check");
const given = args.find((a) => a !== "--check");

// Plain MAJOR.MINOR.PATCH only: Windows installers (MSI) refuse anything else.
const m = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(given ?? "");
if (!m) {
  console.error(
    `Give a version like v0.1.3 (got ${given ? `"${given}"` : "nothing"}). ` +
      "Only MAJOR.MINOR.PATCH numbers work: Windows installers refuse anything else.",
  );
  process.exit(2);
}
const version = `${Number(m[1])}.${Number(m[2])}.${Number(m[3])}`;

const edits = {
  "Cargo.toml": (s) => {
    const at = s.indexOf("[workspace.package]");
    if (at < 0) throw new Error("no [workspace.package] section");
    const rest = s.slice(at);
    const line = /^version = "[^"]*"$/m;
    if (!line.test(rest)) throw new Error("no version in [workspace.package]");
    return s.slice(0, at) + rest.replace(line, `version = "${version}"`);
  },
  // Chummy's own crates are the packages without a `source`.
  "Cargo.lock": (s) =>
    s
      .split("\n[[package]]\n")
      .map((block, i) =>
        i > 0 && !/^source = /m.test(block) && /^name = "chummy/m.test(block)
          ? block.replace(/^version = "[^"]*"$/m, `version = "${version}"`)
          : block,
      )
      .join("\n[[package]]\n"),
  "src-tauri/tauri.conf.json": (s) => topVersion(s),
  "package.json": (s) => topVersion(s),
  // The project's own entry under "packages" too.
  "package-lock.json": (s) =>
    replaceOnce(topVersion(s), /("packages": \{\s*"": \{[^{}]*?"version": )"[^"]*"/, `$1"${version}"`),
};

function replaceOnce(s, re, to) {
  if (!re.test(s)) throw new Error(`no ${re} to change`);
  return s.replace(re, to);
}

// The top-level "version" line, leaving the rest of the file as it is.
function topVersion(s) {
  return replaceOnce(s, /^  "version": "[^"]*"/m, `  "version": "${version}"`);
}

let differ = 0;
for (const [file, edit] of Object.entries(edits)) {
  const path = join(root, file);
  const before = readFileSync(path, "utf8").replace(/\r\n/g, "\n");
  const after = edit(before);
  if (after === before) continue;
  differ++;
  if (check) console.log(`${file} doesn't say ${version}`);
  else writeFileSync(path, after);
}

if (check) {
  if (differ) process.exit(1);
  console.log(`Every file says ${version}.`);
} else {
  console.log(differ ? `Set the version to ${version} in ${differ} file(s).` : `Already ${version}.`);
}
