#!/usr/bin/env node
// One command that moves the version everywhere it is written down.
//
// # Why this exists
//
// The app version had lived in `tauri.conf.json`, and nothing connected it to
// the tag being released. So `v0.2.0` shipped a build that called itself 0.1.0,
// and — because the updater manifest takes its version from the build, not the
// tag — every install was told it was already up to date. The updater was
// working; it was being handed the wrong number.
//
// `tauri.conf.json` now reads `../package.json`, so this script has two files
// to touch rather than three:
//
//   * `package.json` (and its lockfile) — the source of truth, and what Tauri
//     reads for the bundle and for `latest.json`.
//   * `src-tauri/Cargo.toml` — not used for the bundle any more, but a crate
//     that disagrees with the app it builds is a trap for the next person.
//
// Usage: node scripts/set-version.mjs 0.2.0   (a leading "v" is accepted)

import { readFileSync, writeFileSync } from "node:fs";

const raw = process.argv[2];
if (!raw) {
  console.error("usage: node scripts/set-version.mjs <version>   e.g. 0.2.0 or v0.2.0");
  process.exit(1);
}

const version = raw.replace(/^v/, "");
// Deliberately strict. A version that is not semver reaches the bundler as a
// build failure on Windows only, hours later, with an unrelated message.
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`not a semver version: "${raw}"`);
  process.exit(1);
}

function editJson(path, mutate) {
  const json = JSON.parse(readFileSync(path, "utf8"));
  mutate(json);
  writeFileSync(path, JSON.stringify(json, null, 2) + "\n");
}

editJson("package.json", (j) => {
  j.version = version;
});

// The lockfile repeats the version, in two places for the root package.
try {
  editJson("package-lock.json", (j) => {
    j.version = version;
    if (j.packages?.[""]) j.packages[""].version = version;
  });
} catch (e) {
  if (e.code !== "ENOENT") throw e;
}

// Only the `[package]` version — a `version = ` under any dependency table must
// not be touched, which is why this anchors on the section rather than the key.
const cargoPath = "src-tauri/Cargo.toml";
const cargo = readFileSync(cargoPath, "utf8");
const replaced = cargo.replace(
  /(\[package\][\s\S]*?\n)version\s*=\s*"[^"]*"/,
  `$1version = "${version}"`,
);
if (replaced === cargo) {
  console.error(`could not find a [package] version in ${cargoPath}`);
  process.exit(1);
}
writeFileSync(cargoPath, replaced);

console.log(`version set to ${version}`);
