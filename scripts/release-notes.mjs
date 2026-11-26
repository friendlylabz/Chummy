#!/usr/bin/env node
// A release's notes, from CHANGELOG.md: the section headed with its version.
//
//   node scripts/release-notes.mjs v0.1.5     (or 0.1.5)
//
// Prints the section's text (without its heading). Links to the
// repository's own files (like docs/user/chummyd.md) are made full links to
// them as they are at that version's tag, so they work on the release's page
// (set GITHUB_REPOSITORY, as GitHub Actions does; left as they are without
// it). Fails, saying why, when
// there's no section for that version or it's empty, so the release
// workflow stops before building a release with no notes.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const given = process.argv[2] ?? "";
const m = /^v?(\d+\.\d+\.\d+)$/.exec(given);
if (!m) {
  console.error(`Give a version like v0.1.5 (got ${given ? `"${given}"` : "nothing"}).`);
  process.exit(2);
}
const version = m[1];

const text = readFileSync(join(root, "CHANGELOG.md"), "utf8").replace(/\r\n/g, "\n");
// Sections: "## <heading>" up to the next "## ".
const sections = [];
for (const line of text.split("\n")) {
  const h = /^## (.*)$/.exec(line);
  if (h) sections.push({ heading: h[1].trim(), lines: [] });
  else if (sections.length) sections.at(-1).lines.push(line);
}
const body = (s) =>
  s.lines
    .join("\n")
    .replace(/<!--[\s\S]*?-->/g, "")
    .trim();

// "## v0.1.5", "## 0.1.5", "## v0.1.5 (2026-12-01)", "## v0.1.5 — the Matrix one"…
const heading = new RegExp(`^v?${version.replace(/\./g, "\\.")}(?![\\d.])`);
const section = sections.find((s) => heading.test(s.heading));
if (!section || !body(section)) {
  const unreleased = sections.find((s) => /^unreleased$/i.test(s.heading));
  const hint =
    unreleased && body(unreleased)
      ? ` There are notes under "## Unreleased": rename that heading to "## v${version}".`
      : ` Add a "## v${version}" section saying what changed.`;
  console.error(
    `CHANGELOG.md has ${section ? "an empty section" : "no section"} for v${version}.${hint}`,
  );
  process.exit(1);
}
let notes = body(section);
const repo = process.env.GITHUB_REPOSITORY;
if (repo) {
  notes = notes.replace(/\]\(([^)\s]+)\)/g, (all, target) =>
    /^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)
      ? all
      : `](https://github.com/${repo}/blob/v${version}/${target.replace(/^\.\//, "")})`,
  );
}
process.stdout.write(notes + "\n");
