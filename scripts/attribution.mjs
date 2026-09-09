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
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
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
  const licences = [];
  const packages = bundledPackages();
  // Its own banner, in the same shape as the crate half's rules. Without one
  // the file reads as two documents stapled together.
  lines.push(
    "=".repeat(80),
    `NPM PACKAGES (${packages.length}) — bundled into the frontend`,
    "=".repeat(80),
    "",
  );

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

    // The same layout the crate blocks use, so one reading habit covers both.
    const licence = preferMit ? "MIT License" : declared;
    licences.push([licence, name]);
    lines.push(
      "-".repeat(80),
      licence,
      "-".repeat(80),
      "",
      "Applies to:",
      `  - ${name} ${meta.version}`,
      "",
      readFileSync(join(dir, chosen), "utf8").trimEnd(),
      "",
    );
  }
  return { text: lines.join("\n"), licences };
}

// ----------------------------------------------------------------- rust half

function rustSection() {
  // Written to a file rather than captured from stdout.
  //
  // cargo-about *refuses* to write to a redirected stdout under PowerShell —
  // it exits non-zero telling you to use `--output-file` — because PowerShell
  // mangles the encoding of piped output. Capturing stdout is exactly what
  // `execFileSync` does, so on Windows CI this failed every time while working
  // perfectly on Linux. An absolute path, because the command runs in
  // `src-tauri`.
  const dir = mkdtempSync(join(tmpdir(), "db-query-about-"));
  const file = join(dir, "licences.txt");
  try {
    execFileSync(
      "cargo",
      [
        "about",
        "generate",
        "about.hbs",
        "--target",
        target,
        "--output-file",
        file,
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

  // Read outside the catch above: an ENOENT from *this* would be reported as
  // "cargo-about is not installed", which is the one thing it is not.
  try {
    const text = readFileSync(file, "utf8");
    if (text.trim().length < 1000) {
      throw new Error(
        `cargo about wrote ${text.length} bytes, which cannot be a licence ` +
          "manifest for 300-odd crates.",
      );
    }
    // Exactly one trailing newline. Writing to a file drops the one stdout got,
    // and normalising here keeps the assembled document byte-identical across
    // platforms rather than one blank line different on Windows.
    return `${text.trimEnd()}\n`;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}


// ------------------------------------------------------- checking the output
//
// Everything below exists because of a review of the shipped file (Stage 12
// §17). Each check corresponds to a defect that was in it, and each defect was
// invisible: cargo-about exited 0 every time, and the file looked plausible.

const RULE = "=".repeat(80);

/**
 * Split the rendered crate section into `{ name, crates, text }` blocks.
 *
 * The template emits `RULE / name / RULE / "Applies to:" / list / text`, so
 * splitting on the rule alternates name, body, name, body from index 1.
 */
export function parseBlocks(section) {
  const parts = section.split(`\n${RULE}\n`);
  const preamble = parts.shift();
  const blocks = [];
  for (let i = 0; i + 1 < parts.length; i += 2) {
    const name = parts[i].trim();
    const body = parts[i + 1];
    const m = body.match(/^\nApplies to:\n((?:  - .*\n)+)\n([\s\S]*)$/);
    if (!m) throw new Error(`could not parse the block for "${name}"`);
    blocks.push({
      name,
      // Unique: cargo-about can list the same crate twice under one licence.
      crates: [...new Set(m[1].trimEnd().split("\n").map((l) => l.replace(/^  - /, "")))],
      text: m[2].trimEnd(),
    });
  }
  return { preamble, blocks };
}

export function renderBlocks(blocks) {
  return blocks
    .map(
      (b) =>
        `${RULE}\n${b.name}\n${RULE}\n\nApplies to:\n` +
        `${b.crates.map((c) => `  - ${c}`).join("\n")}\n\n${b.text}\n`,
    )
    .join("\n");
}

/** The crate's name alone, from an "Applies to" line. */
const crateName = (line) => line.split(/\s+/)[0];

/**
 * Merge blocks that say the same thing.
 *
 * `ring`'s Apache text appeared twice differing only in indentation, and
 * `miniz_oxide`'s twice differing by a blank line. Compared on collapsed
 * whitespace, so a reflow cannot hide a duplicate.
 */
export function dedupe(blocks) {
  const seen = new Map();
  for (const b of blocks) {
    const key = `${b.name}\u0000${b.text.replace(/\s+/g, " ").trim()}`;
    const first = seen.get(key);
    if (!first) {
      seen.set(key, b);
      continue;
    }
    for (const c of b.crates) if (!first.crates.includes(c)) first.crates.push(c);
    first.crates.sort();
  }
  return [...seen.values()];
}

/**
 * The crates cargo-about could not find a notice for, and what we say instead.
 *
 * Its fallback is the SPDX *template* — `Copyright (c) <year> <copyright
 * holders>`. MIT's one substantive condition is that the copyright notice be
 * retained, and a literal `<copyright holders>` retains nothing; printing it is
 * worse than saying plainly that there was nothing to retain.
 *
 * Every crate that reaches this point has been checked by hand: each one either
 * publishes no licence file at all, or publishes one that cannot be read (the
 * sqlx crates ship `LICENSE-MIT` as a symlink to `../LICENSE-MIT`, which does
 * not resolve inside the package, so its content is that string). The ones that
 * *do* ship a real notice are named in `about.toml` instead — see the
 * clarifications there.
 */
export const PLACEHOLDER = "Copyright (c) <year> <copyright holders>";

export function explainMissingNotices(blocks) {
  for (const b of blocks) {
    if (!b.text.includes(PLACEHOLDER)) continue;
    b.text = b.text.replace(
      PLACEHOLDER,
      [
        "No copyright notice is reproduced here, because none is published.",
        "",
        "Each crate above either ships no licence file in its crates.io package,",
        "or ships one that cannot be read from it. There is therefore no notice to",
        "retain, and inventing one would be worse than saying so. The licence they",
        "grant is below; the holder of each copyright is the project at the URL",
        "beside its name.",
      ].join("\n"),
    );
  }
  return blocks;
}

/** Crates we told `about.toml` to clarify, which must therefore be clarified. */
function clarifiedCrates() {
  const toml = readFileSync("src-tauri/about.toml", "utf8");
  return [...toml.matchAll(/^\[([A-Za-z0-9_.-]+)\.clarify\]/gm)].map((m) => m[1]);
}

/**
 * Fail the build on any defect the review found.
 *
 * The point is not tidiness. A clarification goes stale **silently** when a
 * crate is bumped — the reason is logged at `debug`, which nothing reads, and
 * generation exits 0 with the wrong output. That is exactly how cargo-about's
 * own built-in `ring` workaround came to do nothing at all while appearing to
 * be in force.
 */
export function verify(blocks) {
  const problems = [];

  const missing = new Set(
    blocks.flatMap((b) => (b.text.includes(PLACEHOLDER) ? b.crates.map(crateName) : [])),
  );
  for (const crate of clarifiedCrates()) {
    if (missing.has(crate)) {
      problems.push(
        `${crate} is clarified in about.toml but still has no notice — the ` +
          "clarification has gone stale (a version bump changes the checksum). " +
          "Re-check the path and SHA-256 against the crate as published now.",
      );
    }
  }

  // Licence *notices*, not source files. cargo-about extracts whole files whose
  // header happens to match a licence, which is how `eddsa_digest()` and a set
  // of C typedefs came to be in a document of legal notices.
  for (const b of blocks) {
    const code = b.text.match(/^\s*(#include|pub fn |fn main|mod \w+;|typedef |static const )/m);
    if (code) {
      problems.push(
        `the "${b.name}" block covering ${b.crates[0]} contains source code ` +
          `(${JSON.stringify(code[1])}), not a licence notice`,
      );
    }
  }

  if (blocks.some((b) => b.crates.some((c) => crateName(c) === "db-query"))) {
    problems.push("db-query lists itself in its own third-party licence file");
  }

  if (problems.length) {
    throw new Error(
      `the generated licence file has ${problems.length} defect(s):\n\n` +
        problems.map((p) => `  * ${p}`).join("\n\n") +
        "\n",
    );
  }
}

/**
 * One count per licence, over **distinct crates**.
 *
 * cargo-about's own overview counts entries, not crates: `ring` alone was
 * eighteen of the eighteen "ISC License", for three actually-ISC crates.
 */
export function counts(blocks, npmLicences) {
  const byLicence = new Map();
  for (const b of blocks) {
    const set = byLicence.get(b.name) ?? new Set();
    for (const c of b.crates) set.add(crateName(c));
    byLicence.set(b.name, set);
  }
  for (const [licence, pkg] of npmLicences) {
    const set = byLicence.get(licence) ?? new Set();
    set.add(pkg);
    byLicence.set(licence, set);
  }
  return [...byLicence.entries()]
    .sort((a, b) => b[1].size - a[1].size || a[0].localeCompare(b[0]))
    .map(([name, set]) => `${name}: ${set.size}`)
    .join(", ");
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

Several dozen entries are in that category, not a handful: everything reached
only through a proc macro. cssparser, cssparser-macros, dtoa-short and selectors
(via dom_query -> tauri-utils -> tauri-codegen -> tauri-macros) are the ones
traced by hand, and syn, quote, proc-macro2, serde_derive, darling, heck,
strsim, cargo_metadata, tauri-codegen, tauri-macros, phf_macros and walkdir are
plainly the same case. They are listed anyway, for the reason above.

A NOTE ON WHAT IS REPRODUCED
----------------------------
Each block below is the licence text as the crate publishes it, not a text we
compose. Three consequences worth stating, because they look like defects and
are not:

  * Some crates' own LICENSE-MIT carries no copyright line — that is their
    authors' convention, and reproducing what they publish means reproducing
    that. Where a crate publishes no readable licence file at all, the block
    says so in place of a copyright notice rather than printing a template.
  * ring's Apache-2.0 block ends with the Go and Chromium BSD-3 notices,
    because ring ships them inside that file. They are part of ring's notice.
  * A crate can appear under more than one licence when it publishes more than
    one that applies.

MOZILLA PUBLIC LICENSE 2.0
--------------------------
MPL-2.0 is file-level copyleft: the source of the covered files must remain
available. One MPL-2.0 crate is genuinely linked into this binary:

  option-ext 0.2.0 — https://github.com/soc/option-ext
  reached via dirs-sys -> dirs -> tauri

Its source is available at that address and under the terms reproduced below.

================================================================================

`;

// Guarded so `scripts/attribution.test.mjs` can import the checks above
// without generating anything: importing this file used to run the build.
if (import.meta.main) {
  process.stdout.write(`Resolving crates for ${target}…\n`);
  const rust = rustSection();
  process.stdout.write("Reading the bundle's sourcemap…\n");
  const npm = npmSection();

  // cargo-about's output is a draft, not the document. Merge what it said twice,
  // say plainly where it had nothing to say, and refuse to write the file at all
  // if any of the defects the last review found have come back.
  const { blocks } = parseBlocks(rust);
  const cleaned = explainMissingNotices(dedupe(blocks));
  verify(cleaned);

  const crateBanner = [
    "=".repeat(80),
    `CRATES (${cleaned.reduce((n, b) => n + b.crates.length, 0)}) — linked into the binary`,
    "=".repeat(80),
    "",
  ].join("\n");

  const overview = `LICENCES, BY DISTINCT PACKAGE\n${counts(cleaned, npm.licences)}\n`;

  writeFileSync(
    out,
    `${header}${overview}\n${crateBanner}\n${renderBlocks(cleaned)}\n${npm.text}\n`,
  );
  process.stdout.write(`wrote ${out}\n`);
}
