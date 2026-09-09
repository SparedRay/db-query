// The checks that decide whether a licence file is fit to ship.
//
// Every case here is a defect that was in the file we published, found by a
// review rather than by anything in this repository. None of them failed
// anything: cargo-about exited 0, CI was green, and the file looked plausible.
// That is what these are for.
//
// Run with `node --test scripts/attribution.test.mjs`.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  PLACEHOLDER, counts, dedupe, explainMissingNotices, parseBlocks, renderBlocks, verify,
} from "./attribution.mjs";

const RULE = "=".repeat(80);

/** A rendered crate section, in the shape the handlebars template emits. */
function section(...blocks) {
  return (
    "preamble\n" +
    blocks
      .map(([name, crates, text]) =>
        `${RULE}\n${name}\n${RULE}\n\nApplies to:\n${crates.map((c) => `  - ${c}`).join("\n")}\n\n${text}\n`)
      .map((b) => `\n${b}`)
      .join("")
  );
}

const parse = (...blocks) => parseBlocks(section(...blocks)).blocks;

test("a rendered section round-trips through parse and render", () => {
  const blocks = parse(
    ["MIT License", ["a 1.0  (http://a)", "b 2.0  (http://b)"], "MIT text"],
    ["ISC License", ["c 3.0  (http://c)"], "ISC text"],
  );
  assert.equal(blocks.length, 2);
  assert.deepEqual(blocks[0].crates, ["a 1.0  (http://a)", "b 2.0  (http://b)"]);
  assert.equal(blocks[1].text, "ISC text");
  assert.match(renderBlocks(blocks), /Applies to:\n {2}- c 3\.0/);
});

/**
 * `ring`'s Apache text appeared twice differing only in indentation, and
 * `miniz_oxide`'s twice differing by one blank line.
 */
test("blocks saying the same thing are merged, whatever their whitespace", () => {
  const merged = dedupe(parse(
    ["MIT License", ["a 1.0"], "Permission is\nhereby granted"],
    ["MIT License", ["b 2.0"], "  Permission  is\n\n  hereby granted  "],
    ["MIT License", ["c 3.0"], "A different licence text entirely"],
  ));
  assert.equal(merged.length, 2, "the two identical blocks did not merge");
  assert.deepEqual(merged[0].crates, ["a 1.0", "b 2.0"]);
});

/** cargo-about can list the same crate twice under one licence. */
test("a crate listed twice in one block appears once", () => {
  const [block] = parse(["MIT License", ["a 1.0", "a 1.0", "b 2.0"], "text"]);
  assert.deepEqual(block.crates, ["a 1.0", "b 2.0"]);
});

/**
 * The biggest defect. MIT's one substantive condition is that the copyright
 * notice be retained, and `<copyright holders>` retains nothing.
 */
test("the template's placeholder copyright is replaced by a statement of fact", () => {
  const [block] = explainMissingNotices(parse(
    ["MIT License", ["unic-common 0.9.0"], `MIT License\n\n${PLACEHOLDER}\n\nPermission is hereby granted`],
  ));
  assert.ok(!block.text.includes(PLACEHOLDER), "the placeholder survived");
  assert.match(block.text, /No copyright notice is reproduced here, because none is published/);
  // The licence they actually grant still has to be there.
  assert.match(block.text, /Permission is hereby granted/);
});

test("a real copyright notice is left exactly as the crate published it", () => {
  const text = "MIT License\n\nCopyright (c) 2019 Tokio Contributors\n\nPermission is hereby granted";
  const [block] = explainMissingNotices(parse(["MIT License", ["tracing-core 0.1.36"], text]));
  assert.equal(block.text, text);
});

// ----------------------------------------------------------------- verify

test("source code in a licence block fails the build", () => {
  // `ring` shipped `eddsa_digest()` and C typedefs into a document of legal
  // notices; `schemars_derive` carried a copy of regex-syntax's `escape()`.
  for (const code of ["pub fn escape(s: &str) {}", "#include <stdint.h>", "typedef struct x y;"]) {
    assert.throws(
      () => verify(parse(["ISC License", ["ring 0.17.14"], `ISC text\n\n${code}`])),
      /contains source code/,
      `not caught: ${code}`,
    );
  }
});

test("the app listing itself fails the build", () => {
  assert.throws(
    () => verify(parse(["MIT License", ["db-query 0.3.7"], "text"])),
    /db-query lists itself/,
  );
});

/**
 * The check that matters most, because the failure it catches is silent: a
 * clarification stops applying the moment a crate changes the file it ships,
 * and cargo-about logs that at `debug` and exits 0. That is exactly how its own
 * built-in `ring` workaround came to do nothing while appearing to be in force.
 */
test("a clarification that has gone stale fails the build", () => {
  const stale = parse([
    "MIT License",
    ["tauri 2.11.5", "unic-common 0.9.0"],
    `MIT License\n\n${PLACEHOLDER}\n\nPermission is hereby granted`,
  ]);
  assert.throws(() => verify(stale), /tauri is clarified in about.toml but still has no notice/);

  // A crate we never claimed to have clarified is not an error: some crates
  // genuinely publish nothing, and saying so is the honest answer.
  const honest = parse([
    "MIT License",
    ["unic-common 0.9.0"],
    `MIT License\n\n${PLACEHOLDER}\n\nPermission is hereby granted`,
  ]);
  assert.doesNotThrow(() => verify(honest));
});

test("a clean section passes", () => {
  assert.doesNotThrow(() =>
    verify(parse(["MIT License", ["serde 1.0"], "Copyright (c) serde\n\nPermission is hereby granted"])));
});

// ------------------------------------------------------------------ counts

/**
 * cargo-about's own overview counts entries, not crates: `ring` alone was
 * eighteen of the eighteen "ISC License", for three crates that are ISC.
 */
test("licences are counted by distinct package, across both halves", () => {
  const blocks = dedupe(parse(
    ["ISC License", ["ring 0.17.14"], "one"],
    ["ISC License", ["ring 0.17.14", "untrusted 0.9.0"], "two"],
    ["MIT License", ["serde 1.0"], "three"],
  ));
  const line = counts(blocks, [["MIT License", "@codemirror/state"]]);
  assert.match(line, /ISC License: 2/, `ring counted more than once: ${line}`);
  assert.match(line, /MIT License: 2/, `the npm half was not counted: ${line}`);
});
