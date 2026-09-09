// What `set-version` has to get right, since a release is the only place it
// runs and a release is a bad place to find out.
//
// Run with `node --test scripts/`, which is what `mise run test` does.
//
// Each case works in a temporary tree rather than in the repository: the script
// takes its paths relative to the working directory, so a test that got this
// wrong would rewrite the real `package.json` while claiming to pass.

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const SCRIPT = resolve(import.meta.dirname, "set-version.mjs");

const CARGO = `[package]
name = "db-query"
version = "0.3.4"
description = "Lightweight MySQL client"
edition = "2021"

[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1" }
`;

/** A throwaway tree shaped like this repository's. */
function tree(pkgVersion = "0.3.4", cargo = CARGO) {
  const dir = mkdtempSync(join(tmpdir(), "set-version-"));
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "db-query", version: pkgVersion }, null, 2) + "\n");
  writeFileSync(
    join(dir, "package-lock.json"),
    JSON.stringify({ name: "db-query", version: pkgVersion, packages: { "": { version: pkgVersion } } }, null, 2) + "\n",
  );
  mkdirSync(join(dir, "src-tauri"));
  writeFileSync(join(dir, "src-tauri", "Cargo.toml"), cargo);
  return dir;
}

/** Returns `{ status, stderr }` rather than throwing, so a failure is a value. */
function run(dir, args = []) {
  try {
    const stdout = execFileSync(process.execPath, [SCRIPT, ...args], {
      cwd: dir,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { status: 0, stdout, stderr: "" };
  } catch (e) {
    return { status: e.status, stdout: e.stdout ?? "", stderr: e.stderr ?? "" };
  }
}

const read = (dir, ...p) => readFileSync(join(dir, ...p), "utf8");

/**
 * The one that failed a release.
 *
 * Releases are cut with `npm version`, so the tagged commit already carries the
 * number and CI stamps a version the files already have. The check used to ask
 * "did the text change?", which for that no-op rewrite is "no" — and it
 * reported that as "could not find a [package] version", on both platforms.
 */
test("stamping a version the tree already has is not an error", (t) => {
  const dir = tree("0.3.4");
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const first = run(dir, ["v0.3.4"]);
  assert.equal(first.status, 0, first.stderr);
  assert.match(read(dir, "src-tauri", "Cargo.toml"), /^version = "0\.3\.4"$/m);

  // And again, because idempotence that only holds once is not idempotence.
  assert.equal(run(dir, ["v0.3.4"]).status, 0);
});

test("a leading v is accepted, and every file moves together", (t) => {
  const dir = tree("0.3.4");
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  assert.equal(run(dir, ["v1.2.3"]).status, 0);
  assert.equal(JSON.parse(read(dir, "package.json")).version, "1.2.3");
  const lock = JSON.parse(read(dir, "package-lock.json"));
  assert.equal(lock.version, "1.2.3");
  assert.equal(lock.packages[""].version, "1.2.3", "the lockfile repeats it, and both copies count");
  assert.match(read(dir, "src-tauri", "Cargo.toml"), /^version = "1\.2\.3"$/m);
});

test("sync mode writes Cargo.toml alone, because npm owns the rest", (t) => {
  const dir = tree("2.0.0");
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const before = read(dir, "package-lock.json");
  assert.equal(run(dir).status, 0);
  assert.match(read(dir, "src-tauri", "Cargo.toml"), /^version = "2\.0\.0"$/m);
  assert.equal(read(dir, "package-lock.json"), before, "sync mode must not fight `npm version`");
});

/**
 * A dependency's `version = "2"` is the same six characters as the one being
 * stamped. Rewriting it would build against whatever tauri release happened to
 * match the app's version number.
 */
test("a dependency's version is never touched", (t) => {
  const dir = tree("0.3.4");
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  assert.equal(run(dir, ["9.9.9"]).status, 0);
  const cargo = read(dir, "src-tauri", "Cargo.toml");
  assert.match(cargo, /^version = "9\.9\.9"$/m);
  assert.match(cargo, /tauri = \{ version = "2"/, "a dependency was stamped");
});

/**
 * The error the old check was reporting by mistake still has to be reachable —
 * a `[package]` with no version of its own must not reach into the next table
 * and stamp a dependency's.
 */
test("a Cargo.toml with no [package] version fails, rather than stamping a dependency", (t) => {
  const dir = tree("0.3.4", '[package]\nname = "db-query"\n\n[dependencies]\ntauri = { version = "2" }\n');
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const out = run(dir, ["1.0.0"]);
  assert.equal(out.status, 1);
  assert.match(out.stderr, /could not find a \[package\] version/);
  assert.match(read(dir, "src-tauri", "Cargo.toml"), /tauri = \{ version = "2" \}/);
});

test("a version that is not semver is refused before anything is written", (t) => {
  const dir = tree("0.3.4");
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  for (const bad of ["v1.2", "latest", "1.2.3.4", ""]) {
    const out = run(dir, [bad]);
    assert.equal(out.status, 1, `"${bad}" should have been refused`);
    assert.match(out.stderr, /not a semver version/);
  }
  assert.equal(JSON.parse(read(dir, "package.json")).version, "0.3.4", "a refused version still wrote something");
});
