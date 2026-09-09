#!/usr/bin/env node
// Build THIRD-PARTY-LICENSES.txt — the notices this app is obliged to ship.
//
// # Why this is not optional
//
// MIT, BSD, ISC and Apache-2.0 impose essentially one condition between them:
// if you distribute a binary, the copyright notice and licence text travel with
// it. Not a link, not the source repository — the text, inside the thing people
// install. Stage 5 started distributing installers, which turned the licence
// audit from a claim about a source tree into a live obligation.
//
// # Two halves, measured differently
//
//   * **Rust** — `cargo about` resolves the real dependency graph for one
//     target. Run per target: the Windows build links ~24 `windows-*` crates
//     the Linux build does not, so a Linux-generated file inside setup.exe
//     would be wrong rather than merely short.
//   * **npm** — taken from the **bundle's own sourcemap**, not from
//     `package.json`. Vite tree-shakes, and a declared dependency is not
//     evidence of shipped code: `@tauri-apps/plugin-updater` is a dependency
//     and contributes nothing, because the updater is driven entirely from
//     Rust.
//
// Usage: node scripts/attribution.mjs [--target <triple>] [--out <path>]

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(name);
  return i === -1 ? fallback : args[i + 1];
};

const target = flag("--target", process.platform === "win32"
  ? "x86_64-pc-windows-msvc"
  : "x86_64-unknown-linux-gnu");
const out = flag("--out", "src-tauri/THIRD-PARTY-LICENSES.txt");

// ------------------------------------------------------------------ npm half

/**
 * Which packages actually contribute code to the shipped bundle.
 *
 * The sourcemap lists every source file vite pulled in, so this is a
 * measurement of the artefact rather than a reading of the manifest.
 */
function bundledPackages() {
  const dir = "dist/assets";
  if (!existsSync(dir)) {
    throw new Error("dist/ is not built — run `npm run build` first");
  }
  const maps = readdirSync(dir).filter((f) => f.endsWith(".js.map"));
  if (maps.length === 0) {
    throw new Error(
      "no sourcemap in dist/assets — this script needs one to see what shipped",
    );
  }

  const names = new Set();
  for (const m of maps) {
    const { sources = [] } = JSON.parse(readFileSync(join(dir, m), "utf8"));
    for (const src of sources) {
      const i = src.indexOf("node_modules/");
      if (i === -1) continue;
      const parts = src.slice(i + "node_modules/".length).split("/");
      names.add(parts[0].startsWith("@") ? `${parts[0]}/${parts[1]}` : parts[0]);
    }
  }
  return [...names].sort();
}

function npmSection() {
  const lines = [];
  const packages = bundledPackages();
  lines.push(`NPM PACKAGES (${packages.length})`, "");

  for (const name of packages) {
    const dir = join("node_modules", name);
    const meta = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));

    // Dual licences resolve the same way the Rust half does: MIT first.
    const declared = String(meta.license ?? "");
    const preferMit = /\bMIT\b/.test(declared);

    const licenceFiles = readdirSync(dir).filter((f) => /^LICEN[SC]E/i.test(f));
    if (licenceFiles.length === 0) {
      // Never guess licence text. A package that ships none is a thing to fix
      // by hand, not to paper over with a template.
      throw new Error(
        `${name} ships no licence file; its text cannot be reproduced. ` +
          `Add a clarification to this script rather than omitting it.`,
      );
    }
    const chosen =
      (preferMit && licenceFiles.find((f) => /MIT/i.test(f))) ||
      licenceFiles.find((f) => !/APACHE/i.test(f)) ||
      licenceFiles[0];

    lines.push(
      "=".repeat(80),
      `${name} ${meta.version}  —  ${preferMit ? "MIT" : declared}`,
      "=".repeat(80),
      "",
      readFileSync(join(dir, chosen), "utf8").trimEnd(),
      "",
    );
  }
  return lines.join("\n");
}

// ----------------------------------------------------------------- rust half

function rustSection() {
  try {
    return execFileSync(
      "cargo",
      [
        "about",
        "generate",
        "about.hbs",
        "--target",
        target,
        // A crate whose licence cannot be resolved is precisely the case this
        // file exists to cover. Failing is the point.
        "--fail",
      ],
      { cwd: "src-tauri", encoding: "utf8", maxBuffer: 128 * 1024 * 1024 },
    );
  } catch (err) {
    if (err.code === "ENOENT" || /no such command/.test(String(err.stderr))) {
      throw new Error(
        "cargo-about is not installed, and a bundle must not be built without " +
          "its licence notices.\n\n" +
          "    cargo install cargo-about --locked --features cli\n",
      );
    }
    throw new Error(`cargo about failed:\n${err.stderr ?? err.message}`);
  }
}

// ------------------------------------------------------------------- assemble

const header = `db-query — third-party licences
================================================================================

This file lists everything linked into or bundled with this build of db-query,
with the licence text each one requires to travel with it.

Built for target: ${target}

db-query itself is MIT — see the LICENSE file beside this one, or
https://github.com/SparedRay/db-query.

A NOTE ON SCOPE
---------------
The crate list is resolved from the real dependency graph for the target above,
so it describes this build rather than the lockfile. It is deliberately a
SUPERSET: a handful of crates reach the build only through proc macros, which
run at compile time and are not linked into the binary. cargo-about cannot tell
those apart from linked dependencies, and over-listing is the safe direction —
naming something we do not ship costs nothing, while omitting something we do
ship is the failure this file exists to prevent.

Known to be build-time only: cssparser, cssparser-macros, dtoa-short, selectors
(all reached via dom_query -> tauri-utils -> tauri-codegen -> tauri-macros, a
proc-macro crate).

MOZILLA PUBLIC LICENSE 2.0
--------------------------
MPL-2.0 is file-level copyleft: the source of the covered files must remain
available. One MPL-2.0 crate is genuinely linked into this binary:

  option-ext 0.2.0 — https://github.com/soc/option-ext
  reached via dirs-sys -> dirs -> tauri

Its source is available at that address and under the terms reproduced below.

================================================================================

`;

process.stdout.write(`Resolving crates for ${target}…\n`);
const rust = rustSection();
process.stdout.write("Reading the bundle's sourcemap…\n");
const npm = npmSection();

writeFileSync(out, `${header}${rust}\n\n${npm}\n`);
process.stdout.write(`wrote ${out}\n`);
