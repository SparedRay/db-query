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
// Two ways to call it:
//
//   node scripts/set-version.mjs 0.3.2   Set everything to this version.
//   node scripts/set-version.mjs         SYNC: take package.json's version and
//                                        write it into Cargo.toml.
//
// Sync mode exists for `npm version`, which bumps package.json and the lockfile
// itself and then runs this as its `version` lifecycle script. In that flow npm
// owns those two files, so touching them here would only fight it — sync mode
// therefore writes Cargo.toml alone.
//
// A leading "v" is accepted in either form.

import { readFileSync, writeFileSync } from "node:fs";

const raw = process.argv[2];
const sync = raw === undefined;
const version = (sync ? JSON.parse(readFileSync("package.json", "utf8")).version : raw).replace(
  /^v/,
  "",
);
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

if (!sync) {
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
}

// Only the `[package]` version — a `version = ` under any dependency table must
// not be touched, which is why this anchors on the section rather than the key.
// The `(?!^\[)` keeps the search inside `[package]`: without it, a `[package]`
// with no version of its own would reach forward into the next table and stamp
// a dependency's.
const PACKAGE_VERSION = /(\[package\]\r?\n(?:(?!^\[)[^\n]*\r?\n)*?)version(\s*=\s*)"[^"]*"/m;

const cargoPath = "src-tauri/Cargo.toml";
const cargo = readFileSync(cargoPath, "utf8");
// Asked as "did the pattern match?", never as "did the text change?".
//
// Those are the same question only while the tree is behind the tag. Since
// releases are cut with `npm version`, the tagged commit already carries the
// number, so CI stamps a version Cargo.toml **already has** — a no-op rewrite
// that the old check reported as "could not find a [package] version", failing
// the release on both platforms. See the release workflow's own note: "CI then
// stamps the same value and changes nothing."
if (!PACKAGE_VERSION.test(cargo)) {
  console.error(`could not find a [package] version in ${cargoPath}`);
  process.exit(1);
}
writeFileSync(cargoPath, cargo.replace(PACKAGE_VERSION, `$1version$2"${version}"`));

console.log(sync ? `Cargo.toml synced to ${version}` : `version set to ${version}`);
