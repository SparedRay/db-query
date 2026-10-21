//! The dictionary: what a database's tables and columns *mean*.
//!
//! The assistant and the MCP server both know a schema's shape and nothing
//! about its meaning. A `status` column holding `P`, `S` and `X` is a guess
//! waiting to happen. This is the file somebody writes to stop that.
//!
//! # A path, not a copy
//!
//! The file lives where the schema it describes lives — in a repository, on a
//! branch, reviewed like anything else — and a connection holds its **path**.
//! Stage 17's rule for Flyway projects, for the same reasons. It also makes
//! reuse free rather than a copy: several connections may point at one file, so
//! dev, UAT and production share a dictionary and one edit lands everywhere.
//!
//! # Keyed by table, because environments differ by database
//!
//! Entries are keyed by table name alone, so one file serves `flyway_dev` and
//! `flyway_qa` — the same schema under two names, which is the reported setup.
//! `[databases.<name>.tables.<table>]` overrides `[tables.<table>]` for a
//! database that has genuinely diverged.
//!
//! # No secret, by construction
//!
//! Nothing here has a field that could hold one, and nothing in this module
//! writes a file. It reads prose somebody chose to write down.
//!
//! # What this does **not** do
//!
//! It does not validate against the schema at load time. A note for a table
//! that does not exist is a fact to report ([`Dictionary::unmatched`]), not a
//! reason to refuse the file: the file is edited while connected to one
//! database and used against three.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The version this build writes and fully understands.
pub const FILE_VERSION: u32 = 1;

/// What one table's entry can say.
///
/// `join` is its own field rather than prose inside `note` because it is the
/// one thing a model gets wrong in a way that looks right: a plausible join on
/// the wrong column returns rows, and nobody checks rows that arrive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableNote {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    /// Column name → what it means. Sparse on purpose: §2's measurement is
    /// that annotating every column doubles the prompt, and only the confusing
    /// ones are worth the tokens.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub columns: BTreeMap<String, String>,
}

impl TableNote {
    /// Nothing worth sending. An entry somebody started and left empty — the
    /// shape a generated stub arrives in — must not count as described.
    pub fn is_empty(&self) -> bool {
        blank(&self.note) && blank(&self.join) && self.columns.values().all(|v| v.trim().is_empty())
    }

    /// Merge an override over this entry. The override wins field by field, so
    /// a per-database entry that only fixes one column keeps the rest.
    fn overlaid_with(&self, other: &TableNote) -> TableNote {
        let mut columns = self.columns.clone();
        for (k, v) in &other.columns {
            columns.insert(k.clone(), v.clone());
        }
        TableNote {
            note: other.note.clone().or_else(|| self.note.clone()),
            join: other.join.clone().or_else(|| self.join.clone()),
            columns,
        }
    }
}

fn blank(s: &Option<String>) -> bool {
    s.as_deref().map(str::trim).unwrap_or("").is_empty()
}

/// One database's overrides.
#[derive(Debug, Clone, Default, Deserialize)]
struct DatabaseScope {
    #[serde(default)]
    tables: BTreeMap<String, TableNote>,
}

/// The file, as `serde` reads it.
///
/// **Unknown keys are ignored**, which is `flyway.rs`'s rule and for its
/// reason: validating the file would reject a dictionary that works. A file
/// written for a later version of this app still loads in this one.
#[derive(Debug, Clone, Default, Deserialize)]
struct Raw {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tables: BTreeMap<String, TableNote>,
    #[serde(default)]
    databases: BTreeMap<String, DatabaseScope>,
}

/// A loaded dictionary.
///
/// Every key is folded to lower case on the way in, and every lookup folds
/// too. `editor.ts` already learned why: *"`FROM Users` found nothing when the
/// server had said `users`"* — MySQL folds table names to lower case on
/// Windows and macOS, and people type them however they like everywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dictionary {
    /// The whole database in a sentence — "money is in cents; dates are UTC".
    pub note: Option<String>,
    tables: BTreeMap<String, TableNote>,
    databases: BTreeMap<String, BTreeMap<String, TableNote>>,
}

/// What reading a file produced.
///
/// A warning is not an error: a dictionary from a future version loads, and
/// says so. Only a file that cannot be read or parsed fails.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub dictionary: Dictionary,
    pub warning: Option<String>,
}

fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

fn fold_tables(src: BTreeMap<String, TableNote>) -> BTreeMap<String, TableNote> {
    src.into_iter()
        .map(|(name, mut entry)| {
            entry.columns = entry
                .columns
                .into_iter()
                .map(|(c, v)| (fold(&c), v))
                .collect();
            (fold(&name), entry)
        })
        .collect()
}

/// Parse a dictionary from TOML text.
pub fn parse(text: &str) -> Result<Loaded, String> {
    let raw: Raw =
        toml::from_str(text).map_err(|e| format!("This is not a readable dictionary: {e}"))?;

    // Newer than we know: read what we understand and say so. Refusing would
    // mean a colleague's editor can lock everyone else out of their own file.
    let warning = match raw.version {
        Some(v) if v > FILE_VERSION => Some(format!(
            "This dictionary says version {v}; this build understands {FILE_VERSION}. \
             Anything newer in it is ignored."
        )),
        _ => None,
    };

    Ok(Loaded {
        dictionary: Dictionary {
            note: raw.note.filter(|n| !n.trim().is_empty()),
            tables: fold_tables(raw.tables),
            databases: raw
                .databases
                .into_iter()
                .map(|(db, scope)| (fold(&db), fold_tables(scope.tables)))
                .collect(),
        },
        warning,
    })
}

/// Where in the text `toml` says the parse broke, as byte offsets.
///
/// **Verified from source**, not guessed: `toml` 1.1.5,
/// `src/de/error.rs:69` — `pub fn span(&self) -> Option<Range<usize>>`
/// (read 2026-10-21). `None` when the error carries no span, and the caller
/// then squiggles the whole buffer rather than line 1, which would point at
/// text that is probably fine.
pub fn error_span(text: &str) -> Option<(usize, usize)> {
    let err = toml::from_str::<toml::Value>(text).err()?;
    let span = err.span()?;
    // A zero-width span cannot be drawn, and an editor given one shows
    // nothing at all — which reads as "no error".
    let end = if span.end > span.start {
        span.end
    } else {
        (span.start + 1).min(text.len())
    };
    Some((span.start, end))
}

/// Read and parse one file.
pub fn load(path: &Path) -> Result<Loaded, String> {
    let text = crate::files::read_file(path).map_err(|e| {
        format!(
            "The dictionary at {} could not be read: {e}",
            path.display()
        )
    })?;
    parse(&text.contents)
}

impl Dictionary {
    /// What this dictionary says about one table in one database.
    ///
    /// The per-database entry wins field by field over the shared one, so an
    /// override that fixes a single column keeps everything else.
    pub fn table(&self, database: &str, table: &str) -> Option<TableNote> {
        let key = fold(table);
        let shared = self.tables.get(&key);
        let scoped = self
            .databases
            .get(&fold(database))
            .and_then(|t| t.get(&key));
        let merged = match (shared, scoped) {
            (Some(a), Some(b)) => a.overlaid_with(b),
            (Some(a), None) => a.clone(),
            (None, Some(b)) => b.clone(),
            (None, None) => return None,
        };
        (!merged.is_empty()).then_some(merged)
    }

    /// What it says about one column. Convenience over [`Dictionary::table`],
    /// for the tree's dots, which ask per column.
    pub fn column(&self, database: &str, table: &str, column: &str) -> Option<String> {
        let entry = self.table(database, table)?;
        entry
            .columns
            .get(&fold(column))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Does this table have anything to say — a note, a join, or any column?
    /// The tree's dot, in one call.
    pub fn describes(&self, database: &str, table: &str) -> bool {
        self.table(database, table).is_some()
    }

    /// Is this dictionary empty of everything?
    pub fn is_empty(&self) -> bool {
        self.note.is_none()
            && self.tables.values().all(TableNote::is_empty)
            && self
                .databases
                .values()
                .all(|t| t.values().all(TableNote::is_empty))
    }

    /// How many tables and columns this dictionary describes, for the tab's
    /// header band. Counted over what is actually said, so a stub somebody
    /// left blank is not a note.
    pub fn counts(&self, database: &str) -> Counts {
        let mut names: Vec<&String> = self.tables.keys().collect();
        if let Some(scoped) = self.databases.get(&fold(database)) {
            names.extend(scoped.keys());
        }
        names.sort_unstable();
        names.dedup();

        let mut counts = Counts::default();
        for name in names {
            if let Some(entry) = self.table(database, name) {
                counts.tables += 1;
                counts.columns += entry
                    .columns
                    .values()
                    .filter(|v| !v.trim().is_empty())
                    .count();
            }
        }
        counts
    }

    /// Notes that match nothing in the schema they are being used against.
    ///
    /// **The thing that keeps a dictionary trustworthy a year later.** A typo'd
    /// `[tables.oders]` is otherwise a note about nothing, silently, forever.
    ///
    /// `tables` is every table name in the database. `has_column` answers
    /// whether one table has one column, and `None` means **its columns have
    /// not been loaded** — in which case that table's column notes are not
    /// reported, because "not loaded yet" is not "not there".
    ///
    /// # Why `has_column` and not a list of names
    ///
    /// It was a list first — `Fn(&str) -> Option<&[String]>` — and the app's
    /// schema cache holds `ColumnInfo`, not `String`, so every caller had to
    /// build a throwaway `Vec` to ask the question. The first one written
    /// (a throwaway probe, minutes later) passed an **empty** slice, and every
    /// column note in a perfectly good dictionary came back as stale. A
    /// signature whose easiest wrong answer is "none of them" is the wrong
    /// signature; this one's easiest wrong answer has to be typed on purpose.
    pub fn unmatched<F>(&self, database: &str, tables: &[String], has_column: F) -> Vec<Unmatched>
    where
        F: Fn(&str, &str) -> Option<bool>,
    {
        let known: std::collections::BTreeSet<String> = tables.iter().map(|t| fold(t)).collect();

        let mut names: Vec<&String> = self.tables.keys().collect();
        if let Some(scoped) = self.databases.get(&fold(database)) {
            names.extend(scoped.keys());
        }
        names.sort_unstable();
        names.dedup();

        let mut out = Vec::new();
        for name in names {
            let Some(entry) = self.table(database, name) else {
                continue;
            };
            if !known.contains(name) {
                out.push(Unmatched {
                    table: name.clone(),
                    column: None,
                });
                continue;
            }
            // The table is there, so its columns can be checked — when they
            // have been loaded.
            for col in entry.columns.keys() {
                match has_column(name, col) {
                    // Loaded, and this column is not among them.
                    Some(false) => out.push(Unmatched {
                        table: name.clone(),
                        column: Some(col.clone()),
                    }),
                    Some(true) => {}
                    // Not loaded: nothing is known to be missing yet.
                    None => break,
                }
            }
        }
        out
    }
}

/// How much a dictionary says about one database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub tables: usize,
    pub columns: usize,
}

/// A note with nothing to attach to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unmatched {
    pub table: String,
    /// `None` when the whole table is missing; the column name when the table
    /// is there and the column is not.
    pub column: Option<String>,
}

impl Unmatched {
    /// How it reads in the warning: `oders` or `orders.stock_lvl`.
    pub fn label(&self) -> String {
        match &self.column {
            Some(c) => format!("{}.{}", self.table, c),
            None => self.table.clone(),
        }
    }
}

// ------------------------------------------------------------------- the cache

/// A dictionary, and what the file looked like when it was read.
///
/// **Stage 24's lesson, applied from the start.** Reading and parsing 240KB
/// costs ~18ms — free per prompt, not free per keystroke, and the tree asks
/// per row. So the file is re-read only when its size or modification time has
/// moved.
struct Cached {
    path: PathBuf,
    stamp: Option<(u64, std::time::SystemTime)>,
    loaded: Loaded,
}

static CACHE: std::sync::Mutex<Option<Cached>> = std::sync::Mutex::new(None);

fn stamp_of(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Load, reusing the last read when the file has not moved.
///
/// One slot, not a map: a window looks at one connection at a time, and two
/// connections sharing a dictionary share its path too. A second path simply
/// replaces the slot.
pub fn load_cached(path: &Path) -> Result<Loaded, String> {
    let stamp = stamp_of(path);
    if let Ok(guard) = CACHE.lock() {
        if let Some(held) = guard.as_ref() {
            // An unreadable stamp never matches, so a file we cannot stat is
            // re-read rather than served from a stale copy.
            if held.path == path && stamp.is_some() && held.stamp == stamp {
                return Ok(held.loaded.clone());
            }
        }
    }

    let loaded = load(path)?;
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(Cached {
            path: path.to_path_buf(),
            stamp,
            loaded: loaded.clone(),
        });
    }
    Ok(loaded)
}

/// Forget whatever is held. For tests, and for "the file was just saved".
pub fn forget() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
}

/// A starter file for a database, as a generated stub per table.
///
/// **A blank file is a feature nobody uses.** Every real table arrives as a
/// commented-out section with its own name spelled correctly, so the work is
/// filling blanks rather than typing 385 headers — and a typo'd table name
/// cannot be the first thing that happens.
pub fn starter(database: &str, tables: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# What the tables and columns of `{database}` mean.\n\
         #\n\
         # Read by this app's assistant and by its MCP server, beside the names\n\
         # and types they already see. Point other connections at this same file\n\
         # rather than copying it.\n\
         #\n\
         # Uncomment a section and write what somebody new would need to know.\n\n"
    ));
    out.push_str(&format!("version = {FILE_VERSION}\n"));
    out.push_str("note = \"\"\n");
    for t in tables {
        out.push_str(&format!("\n# [tables.{t}]\n# note = \"\"\n"));
    }
    out
}

// ------------------------------------------------------------------ the merge
//
// Stage 25 slice 5. Something outside the app — an agent with the code in
// front of it — proposes what a table means, and the proposal has to end up in
// a file a person already owns and may be in the middle of editing.
//
// # Text, not a round trip through the parser
//
// Re-serialising a parsed dictionary would be a dozen lines and would destroy
// the file: `starter()` writes every real table as a *commented-out* stub, so
// most of a young dictionary is comments, and `serde` does not keep comments.
// Everything below therefore edits the text in place, which is also what makes
// the result reviewable as a diff.
//
// # Every write is read back
//
// TOML has more ways to spell an entry than this module edits — a
// `[tables.x.columns]` subtable, a `[databases.db.tables.x]` override, an
// inline table. Rather than handle each, [`merge`] writes, re-parses, and
// checks that the dictionary now *says* what was asked. A write that does not
// read back is reverted and reported, so the failure mode is "your note was
// not written, here is why" rather than a corrupted file or a silent loss.

/// The longest note this accepts, in characters.
///
/// A dictionary note is a sentence or two: §2's measurement is that the whole
/// annotation budget for a prompt is 8,000 characters, so one note that
/// spends an eighth of it is a mistake rather than a style.
pub const MAX_NOTE_CHARS: usize = 1_000;

/// What to write about one table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEdit {
    pub table: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub join: Option<String>,
    #[serde(default)]
    pub columns: BTreeMap<String, String>,
}

/// What one table's edit did. **Both halves matter**: `kept` is how a caller
/// learns that a sentence somebody wrote by hand was left alone, which is the
/// difference between "done" and "done, and I overwrote you".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub table: String,
    /// Field labels written: `note`, `join`, `columns.status`.
    pub wrote: Vec<String>,
    /// Field labels left as they were, because they already said something.
    pub kept: Vec<String>,
}

/// A table whose notes were not written at all, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub table: String,
    pub why: String,
}

/// The merged text, and what became of each table.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Merged {
    pub text: String,
    pub applied: Vec<Applied>,
    pub skipped: Vec<Skipped>,
}

/// Write notes into a dictionary's text.
///
/// `database` is only consulted to answer *what the file currently says* — a
/// per-database override has to be seen, or a write would be reported as
/// landing when it is being shadowed. Writes themselves always go to the
/// shared `[tables.<name>]` section, which is the keying the whole file is
/// built on (§ "Keyed by table").
///
/// `replace` decides the one question that is not ours: whether a note
/// somebody already wrote may be overwritten. Default off — the person's own
/// sentence outranks a generated one, and an agent that means it can say so.
pub fn merge(
    text: &str,
    database: &str,
    edits: &[NoteEdit],
    replace: bool,
) -> Result<Merged, String> {
    // A file that does not parse cannot be merged into: every decision below
    // asks what it currently says, and the answer would be a guess.
    parse(text).map_err(|e| {
        format!("The dictionary as it stands cannot be read, so nothing was written: {e}")
    })?;

    let mut out = text.to_string();
    let mut applied = Vec::new();
    let mut skipped = Vec::new();

    for edit in edits {
        let spelled = edit.table.trim();
        if spelled.is_empty() {
            skipped.push(Skipped {
                table: edit.table.clone(),
                why: "No table name was given.".into(),
            });
            continue;
        }
        match apply(&out, database, spelled, edit, replace) {
            Ok((next, done)) => {
                out = next;
                applied.push(done);
            }
            Err(why) => skipped.push(Skipped {
                table: spelled.to_string(),
                why,
            }),
        }
    }

    Ok(Merged {
        text: out,
        applied,
        skipped,
    })
}

/// One table's edit: decide the fields, write them, read them back.
fn apply(
    text: &str,
    database: &str,
    spelled: &str,
    edit: &NoteEdit,
    replace: bool,
) -> Result<(String, Applied), String> {
    let name = fold(spelled);
    let said = parse(text)
        .ok()
        .and_then(|l| l.dictionary.table(database, &name));

    // What was asked for, in spending order: what the table is, then how it
    // joins, then its columns — the order §2 settled for the prompt, and the
    // order a reader of the file wants them in.
    let mut asked: Vec<(Vec<String>, String, String)> = Vec::new();
    let mut want = |path: Vec<String>, label: String, value: Option<&String>| {
        if let Some(v) = value.map(|v| v.trim()).filter(|v| !v.is_empty()) {
            asked.push((path, label, v.to_string()));
        }
    };
    want(vec!["note".into()], "note".into(), edit.note.as_ref());
    want(vec!["join".into()], "join".into(), edit.join.as_ref());
    for (column, value) in &edit.columns {
        let column = column.trim();
        if column.is_empty() {
            continue;
        }
        want(
            vec!["columns".into(), fold(column)],
            format!("columns.{column}"),
            Some(value),
        );
    }

    if asked.is_empty() {
        return Err("Nothing to write: every note in it was empty.".into());
    }
    if let Some((_, label, value)) = asked
        .iter()
        .find(|(_, _, v)| v.chars().count() > MAX_NOTE_CHARS)
    {
        return Err(format!(
            "`{label}` is {} characters; a dictionary note is a sentence or two, and {MAX_NOTE_CHARS} \
             is the limit. Nothing was written for this table.",
            value.chars().count()
        ));
    }

    // Already written, and nobody said to overwrite: kept, and said so.
    let mut wrote = Vec::new();
    let mut kept = Vec::new();
    let mut writing = Vec::new();
    for (path, label, value) in asked {
        let already = existing(said.as_ref(), &path);
        if already.is_some() && !replace {
            kept.push(label);
        } else {
            wrote.push(label);
            writing.push((path, value));
        }
    }

    let done = Applied {
        table: name.clone(),
        wrote,
        kept,
    };
    if writing.is_empty() {
        // Nothing to do, and that is not a failure: the file already says it.
        return Ok((text.to_string(), done));
    }

    let mut out = text.to_string();
    match table_section(&out, &name) {
        Some(_) => {
            for (path, value) in &writing {
                // Re-found each time: every write moves the offsets after it.
                let body = table_section(&out, &name).ok_or_else(|| {
                    "The section disappeared while it was being written.".to_string()
                })?;
                out = set_key(&out, body, path, value);
            }
        }
        None => {
            let section = new_section(spelled, &writing);
            match stub_block(&out, &name) {
                // A generated stub is replaced where it stands. `starter()`
                // writes one per real table, so appending below would leave a
                // young dictionary saying everything twice with the dead copy
                // on top.
                Some((from, to)) => out.replace_range(from..to, &section),
                None => {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if !out.is_empty() && !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                    out.push_str(&section);
                }
            }
        }
    }

    // The write is only a write if the dictionary now says it.
    let got = parse(&out)
        .map_err(|e| {
            format!(
                "Writing this would have left the file unreadable, so nothing was written \
                 for `{name}`. Its entry may use a form this app does not edit — a \
                 `[tables.{name}.columns]` subtable, or an inline table. ({e})"
            )
        })?
        .dictionary
        .table(database, &name);
    for (path, value) in &writing {
        if existing(got.as_ref(), path).as_deref() != Some(value.as_str()) {
            return Err(format!(
                "After writing, the dictionary still reads differently for `{name}`, so \
                 nothing was written: something else in the file decides this entry — a \
                 `[databases.*.tables.{name}]` override, most likely."
            ));
        }
    }

    Ok((out, done))
}

/// What the dictionary currently says at one field path, trimmed, or `None`
/// when it says nothing there.
fn existing(said: Option<&TableNote>, path: &[String]) -> Option<String> {
    let said = said?;
    let value = match path {
        [one] if one == "note" => said.note.as_deref(),
        [one] if one == "join" => said.join.as_deref(),
        [first, column] if first == "columns" => said.columns.get(column).map(String::as_str),
        _ => None,
    }?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

// ------------------------------------------------------------- reading TOML as text

/// Every line, with its byte offset. The newline stays on the line, so
/// splicing one preserves whatever the file uses.
fn text_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        out.push((at, line));
        at += line.len();
    }
    out
}

/// A dotted key at the start of `s`: its folded segments, and how many bytes
/// it took. Quoted segments are read but never written back quoted unless they
/// have to be — this is for *finding* a key.
fn dotted(s: &str) -> Option<(Vec<String>, usize)> {
    let mut rest = s;
    let mut used = 0;
    let mut path = Vec::new();
    loop {
        let trimmed = rest.trim_start_matches([' ', '\t']);
        used += rest.len() - trimmed.len();
        rest = trimmed;

        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
        let segment = match quote {
            Some(q) => {
                let body = &rest[1..];
                // A key with an escaped quote in it is not a key this edits;
                // failing to read it means "no match", which is correct.
                let end = body.find(q)?;
                let segment = body[..end].to_string();
                let taken = 1 + end + q.len_utf8();
                used += taken;
                rest = &rest[taken..];
                segment
            }
            None => {
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(rest.len());
                if end == 0 {
                    return None;
                }
                let segment = rest[..end].to_string();
                used += end;
                rest = &rest[end..];
                segment
            }
        };
        path.push(fold(&segment));

        let trimmed = rest.trim_start_matches([' ', '\t']);
        used += rest.len() - trimmed.len();
        rest = trimmed;
        match rest.strip_prefix('.') {
            Some(after) => {
                used += 1;
                rest = after;
            }
            None => return Some((path, used)),
        }
    }
}

/// Does this line open a section? `[[array]]` counts: it ends the section
/// above it just the same, even though nothing here matches one.
fn is_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// The folded path a `[section]` header names, when it is one this can read.
fn header_path(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    if rest.starts_with('[') {
        return None;
    }
    let (path, used) = dotted(rest)?;
    rest[used..].trim_start().starts_with(']').then_some(path)
}

/// The folded key a line assigns to, and the byte index just past the key's
/// own text — so the key can be rewritten exactly as the user spelled it.
fn key_line(line: &str) -> Option<(Vec<String>, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    if rest.starts_with('#') || rest.starts_with('[') {
        return None;
    }
    let (path, used) = dotted(rest)?;
    rest[used..]
        .trim_start()
        .starts_with('=')
        .then(|| (path, indent + used))
}

/// Where `[tables.<name>]`'s body is: from just after its header line to the
/// start of the next header, or the end of the file.
fn table_section(text: &str, name: &str) -> Option<(usize, usize)> {
    let mut body: Option<usize> = None;
    for (at, line) in text_lines(text) {
        if !is_header(line) {
            continue;
        }
        if let Some(start) = body {
            return Some((start, at));
        }
        if header_path(line).is_some_and(|p| p.len() == 2 && p[0] == "tables" && p[1] == name) {
            body = Some(at + line.len());
        }
    }
    body.map(|start| (start, text.len()))
}

/// The byte range of a generated stub for this table — `# [tables.x]` and the
/// empty `# note = ""` lines under it — when that is what is there.
///
/// Narrow on purpose: only lines whose uncommented form assigns an **empty
/// string** are taken. A commented line somebody wrote themselves is left
/// exactly where it is.
fn stub_block(text: &str, name: &str) -> Option<(usize, usize)> {
    let lines = text_lines(text);
    let mut found: Option<(usize, usize)> = None;
    for (i, (at, line)) in lines.iter().enumerate() {
        let Some(bare) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        if !header_path(bare).is_some_and(|p| p.len() == 2 && p[0] == "tables" && p[1] == name) {
            continue;
        }
        let mut end = at + line.len();
        for (next_at, next) in &lines[i + 1..] {
            let Some(bare) = next.trim_start().strip_prefix('#') else {
                break;
            };
            let empty = key_line(bare).is_some_and(|(_, key_end)| {
                let value = bare[key_end..].trim_start().trim_start_matches('=').trim();
                value == "\"\"" || value == "''"
            });
            if !empty {
                break;
            }
            end = next_at + next.len();
        }
        found = Some((*at, end));
        break;
    }
    found
}

// ------------------------------------------------------------- writing TOML as text

/// Set one key inside a section's body, replacing its line when it is there
/// and adding one after the section's last key when it is not — so a trailing
/// comment or blank line stays below rather than being written over.
fn set_key(text: &str, body: (usize, usize), path: &[String], value: &str) -> String {
    let (start, end) = body;
    let section = &text[start..end];

    let mut after_last_key = 0;
    for (at, line) in text_lines(section) {
        match key_line(line) {
            Some((found, key_end)) if found == path => {
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                let mut out = text.to_string();
                out.replace_range(
                    start + at..start + at + line.len(),
                    // `key_end` is where the key's own text stops, so the
                    // whitespace the file had around `=` is rewritten rather
                    // than doubled.
                    &format!(
                        "{} = {}{newline}",
                        line[..key_end].trim_end(),
                        quoted(value)
                    ),
                );
                return out;
            }
            Some(_) => after_last_key = at + line.len(),
            None => {}
        }
    }

    let at = start + after_last_key;
    let mut insert = String::new();
    if at > 0 && !text[..at].ends_with('\n') {
        insert.push('\n');
    }
    insert.push_str(&format!("{} = {}\n", key_text(path), quoted(value)));
    let mut out = text.to_string();
    out.insert_str(at, &insert);
    out
}

/// A whole `[tables.<name>]` section, for a table the file has never mentioned.
///
/// The name is written as the caller spelled it. Lookups fold, so the case
/// makes no difference to what is found — and a file somebody reads is better
/// off saying `OrderLines` where their schema does.
fn new_section(spelled: &str, fields: &[(Vec<String>, String)]) -> String {
    let mut out = format!(
        "[{}]\n",
        key_text(&[String::from("tables"), spelled.to_string()])
    );
    for (path, value) in fields {
        out.push_str(&format!("{} = {}\n", key_text(path), quoted(value)));
    }
    out
}

/// A dotted key, quoting only the segments that need it.
fn key_text(path: &[impl AsRef<str>]) -> String {
    path.iter()
        .map(|segment| {
            let segment = segment.as_ref();
            if bare_key(segment) {
                segment.to_string()
            } else {
                quoted(segment)
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn bare_key(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A TOML basic string.
///
/// Hand-written rather than borrowed from the serializer because the
/// serializer writes *documents*: there is no "give me one value" entrance to
/// it. The property that matters is tested by reading the result back through
/// `parse` rather than by eye — see `prose_with_quotes_and_newlines_survives`.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // Every other control character, which a basic string may not
            // hold literally. Prose pasted out of a terminal has them.
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        version = 1
        note = "Order management. Money is in cents; every date is UTC."

        [tables.orders]
        note = "One row per customer order. Not per line — see order_lines."
        join = "orders.customer_id = customers.id"
        columns.status = "P=pending, S=shipped, X=cancelled."
        columns.legacy_customer_ref = "Dead since 2024. Never join on this."

        [tables.order_lines]
        note = "One row per line. qty is in base units."

        [databases.warehouse_archive.tables.orders]
        note = "Frozen copy. Nothing writes here."
        columns.status = "Always S in the archive."
    "#;

    fn dict() -> Dictionary {
        parse(SAMPLE).expect("the sample parses").dictionary
    }

    /// D1, the plain case.
    #[test]
    fn a_table_and_a_column_are_found() {
        let d = dict();
        let orders = d.table("warehouse", "orders").expect("orders is described");
        assert_eq!(
            orders.note.as_deref(),
            Some("One row per customer order. Not per line — see order_lines.")
        );
        assert_eq!(
            orders.join.as_deref(),
            Some("orders.customer_id = customers.id")
        );
        assert_eq!(
            d.column("warehouse", "orders", "status").as_deref(),
            Some("P=pending, S=shipped, X=cancelled.")
        );
        assert_eq!(
            d.note.as_deref().unwrap().starts_with("Order management"),
            true
        );
    }

    /// A table nobody wrote about has nothing, and says so rather than
    /// answering with an empty entry.
    #[test]
    fn a_table_nobody_described_has_nothing() {
        let d = dict();
        assert_eq!(d.table("warehouse", "audit_log"), None);
        assert!(!d.describes("warehouse", "audit_log"));
        assert_eq!(d.column("warehouse", "orders", "total_cents"), None);
    }

    /// §3.5. The one that `editor.ts` already paid for once.
    #[test]
    fn the_case_of_a_name_does_not_decide_whether_it_is_found() {
        let d = dict();
        for name in ["orders", "Orders", "ORDERS", "  OrDeRs  "] {
            assert!(d.describes("warehouse", name), "{name} was not found");
        }
        assert_eq!(
            d.column("WAREHOUSE", "Orders", "STATUS").as_deref(),
            Some("P=pending, S=shipped, X=cancelled.")
        );
    }

    /// §3.4: one file, several environments. The *database* name does not
    /// decide whether a shared entry applies.
    #[test]
    fn a_shared_entry_applies_to_every_database() {
        let d = dict();
        for db in ["flyway_dev", "flyway_qa", "anything_at_all"] {
            assert!(d.describes(db, "orders"), "not found in {db}");
        }
    }

    /// And an override wins **field by field**, so fixing one column does not
    /// silently drop the join that was right already.
    #[test]
    fn a_database_override_wins_field_by_field() {
        let d = dict();
        let archive = d.table("warehouse_archive", "orders").expect("described");
        assert_eq!(
            archive.note.as_deref(),
            Some("Frozen copy. Nothing writes here.")
        );
        assert_eq!(
            archive.join.as_deref(),
            Some("orders.customer_id = customers.id"),
            "the shared join survives an override that did not mention it"
        );
        assert_eq!(
            archive.columns.get("status").map(String::as_str),
            Some("Always S in the archive.")
        );
        assert_eq!(
            archive
                .columns
                .get("legacy_customer_ref")
                .map(String::as_str),
            Some("Dead since 2024. Never join on this."),
            "and so do the columns it did not mention"
        );
        // While the other database keeps the shared text.
        assert_eq!(
            d.column("warehouse", "orders", "status").as_deref(),
            Some("P=pending, S=shipped, X=cancelled.")
        );
    }

    /// D2. A file from a later version of this app still loads.
    #[test]
    fn keys_this_build_does_not_know_are_ignored_and_the_version_is_said() {
        let loaded = parse(
            r#"
            version = 9
            note = "still readable"
            glossary = { tenant = "a customer account" }

            [tables.orders]
            note = "readable"
            sensitivity = "internal"
            columns.status = "readable"
            "#,
        )
        .expect("a newer file must still load");

        let w = loaded.warning.expect("and must say it is newer");
        assert!(w.contains("version 9"), "{w}");
        assert!(w.contains("understands 1"), "{w}");
        assert!(loaded.dictionary.describes("any", "orders"));
        assert_eq!(
            loaded
                .dictionary
                .column("any", "orders", "status")
                .as_deref(),
            Some("readable")
        );
    }

    /// A file of this version says nothing about versions.
    #[test]
    fn a_current_file_warns_about_nothing() {
        assert!(parse(SAMPLE).unwrap().warning.is_none());
        assert!(parse("note = \"no version key at all\"")
            .unwrap()
            .warning
            .is_none());
    }

    /// The squiggle lands on the line that broke, not on line 1.
    ///
    /// `toml` carries a span; this asserts it points *inside* the offending
    /// text rather than at the top of the file, because a diagnostic on a
    /// line that is fine is worse than none.
    #[test]
    fn a_parse_error_points_at_the_text_that_broke() {
        let text = "version = 1\n\n[tables.orders]\nnote = \"fine\"\n\n[tables.oops\n";
        let (start, end) = error_span(text).expect("toml gives a span");
        assert!(end > start, "a zero-width span cannot be drawn");
        let broke = text.find("[tables.oops").unwrap();
        assert!(
            start >= broke,
            "the span starts at {start}, before the broken line at {broke}"
        );
        assert!(end <= text.len());
    }

    /// Text that parses has no span to report.
    #[test]
    fn good_toml_has_no_error_span() {
        assert_eq!(error_span("version = 1\n"), None);
    }

    /// Broken TOML is an error with Flyway-style wording — the file is named by
    /// the caller, the reason by `toml`.
    #[test]
    fn unparseable_toml_says_so() {
        let e = parse("[tables.orders\nnote = 1").expect_err("must not parse");
        assert!(e.starts_with("This is not a readable dictionary:"), "{e}");
    }

    /// An empty entry is not a note. Generated stubs arrive looking exactly
    /// like this, and a tree full of dots for tables nobody has described yet
    /// would make the dot meaningless.
    #[test]
    fn a_stub_somebody_left_blank_does_not_count_as_described() {
        let d = parse(
            r#"
            [tables.orders]
            note = ""
            [tables.shipments]
            note = "   "
            columns.carrier = ""
            [tables.customers]
            note = "Real."
            "#,
        )
        .unwrap()
        .dictionary;

        assert!(!d.describes("db", "orders"));
        assert!(!d.describes("db", "shipments"));
        assert!(d.describes("db", "customers"));
        assert_eq!(
            d.counts("db"),
            Counts {
                tables: 1,
                columns: 0
            }
        );
    }

    /// The header band's numbers, over what is actually said.
    #[test]
    fn the_counts_are_what_the_tab_header_shows() {
        assert_eq!(
            dict().counts("warehouse"),
            Counts {
                tables: 2,
                columns: 2
            }
        );
        // The override adds no table and no column — it is the same `orders`.
        assert_eq!(
            dict().counts("warehouse_archive"),
            Counts {
                tables: 2,
                columns: 2
            }
        );
    }

    /// D3. A typo is reported by name, and the loaded/not-loaded distinction is
    /// respected: a table whose columns nobody has fetched reports no columns.
    #[test]
    fn notes_that_match_nothing_are_reported_and_nothing_else_is() {
        let d = dict();
        let tables = vec!["orders".to_string(), "customers".to_string()];
        let orders_cols = ["id", "customer_id", "status"];

        let found = d.unmatched("warehouse", &tables, |t, c| match t {
            "orders" => Some(orders_cols.contains(&c)),
            // `customers` is in the database but its columns are not loaded.
            _ => None,
        });

        let labels: Vec<String> = found.iter().map(Unmatched::label).collect();
        // **Alphabetical by table**, because the warning line that shows these
        // must read the same way twice. Written in that order rather than in
        // the order they happened to come to mind — which is how this
        // expectation was wrong the first time.
        assert_eq!(
            labels,
            [
                // `order_lines` is not a table in this database at all.
                "order_lines",
                // `orders` is; this column of it is not.
                "orders.legacy_customer_ref",
            ]
        );
    }

    /// The case the loaded/not-loaded rule exists for: nothing is reported
    /// about a table whose columns have never been fetched.
    #[test]
    fn unloaded_columns_are_not_reported_as_missing() {
        let d = dict();
        let tables = vec!["orders".to_string(), "order_lines".to_string()];
        let found = d.unmatched("warehouse", &tables, |_, _| None);
        assert!(
            found.is_empty(),
            "nothing is known to be missing yet: {found:?}"
        );
    }

    /// An override's own typo is reported too — it is a note like any other.
    #[test]
    fn an_override_for_a_table_that_is_gone_is_reported() {
        let d = parse(
            r#"
            [databases.archive.tables.oders]
            note = "typo, only in the archive's section"
            "#,
        )
        .unwrap()
        .dictionary;

        let tables = vec!["orders".to_string()];
        assert_eq!(
            d.unmatched("archive", &tables, |_, _| None)
                .iter()
                .map(Unmatched::label)
                .collect::<Vec<_>>(),
            ["oders"]
        );
        // And it is not reported against a database it does not apply to.
        assert!(d.unmatched("live", &tables, |_, _| None).is_empty());
    }

    /// The starter file names every real table, commented out, and is itself a
    /// valid dictionary that describes nothing.
    #[test]
    fn the_starter_is_valid_and_describes_nothing_yet() {
        let text = starter("warehouse", &["orders".into(), "order_lines".into()]);
        assert!(text.contains("# [tables.orders]"));
        assert!(text.contains("# [tables.order_lines]"));
        assert!(text.contains(&format!("version = {FILE_VERSION}")));

        let d = parse(&text).expect("a starter must parse").dictionary;
        assert!(
            d.is_empty(),
            "a starter describes nothing until it is edited"
        );
        assert_eq!(d.counts("warehouse"), Counts::default());
    }

    /// An empty dictionary is empty whichever way it is written.
    #[test]
    fn empty_is_empty() {
        assert!(parse("").unwrap().dictionary.is_empty());
        assert!(parse("version = 1").unwrap().dictionary.is_empty());
        assert!(!parse("note = \"something\"").unwrap().dictionary.is_empty());
    }

    // ----------------------------------------------------------- the cache

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("db-query-dict-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The cache serves an untouched file and notices a changed one.
    ///
    /// Written with the file's mtime set explicitly rather than by waiting: a
    /// test that writes twice in the same millisecond and hopes the timestamp
    /// moved is a test that fails on a fast machine.
    #[test]
    fn the_cache_notices_an_edit() {
        forget();
        let dir = scratch("cache");
        let path = dir.join("dictionary.toml");
        std::fs::write(&path, "[tables.orders]\nnote = \"first\"\n").unwrap();

        let first = load_cached(&path).unwrap().dictionary;
        assert_eq!(
            first.table("db", "orders").unwrap().note.as_deref(),
            Some("first")
        );
        // Same file, same stamp: the second read is the first one.
        assert_eq!(load_cached(&path).unwrap().dictionary, first);

        // An edit of a different length moves the stamp whatever the clock did.
        std::fs::write(&path, "[tables.orders]\nnote = \"second, and longer\"\n").unwrap();
        assert_eq!(
            load_cached(&path)
                .unwrap()
                .dictionary
                .table("db", "orders")
                .unwrap()
                .note
                .as_deref(),
            Some("second, and longer")
        );

        forget();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second path replaces the slot rather than being answered from the
    /// first one's contents.
    #[test]
    fn another_path_is_another_dictionary() {
        forget();
        let dir = scratch("two");
        let a = dir.join("a.toml");
        let b = dir.join("b.toml");
        std::fs::write(&a, "[tables.orders]\nnote = \"from a\"\n").unwrap();
        std::fs::write(&b, "[tables.orders]\nnote = \"from b\"\n").unwrap();

        assert_eq!(
            load_cached(&a)
                .unwrap()
                .dictionary
                .table("d", "orders")
                .unwrap()
                .note
                .as_deref(),
            Some("from a")
        );
        assert_eq!(
            load_cached(&b)
                .unwrap()
                .dictionary
                .table("d", "orders")
                .unwrap()
                .note
                .as_deref(),
            Some("from b")
        );
        assert_eq!(
            load_cached(&a)
                .unwrap()
                .dictionary
                .table("d", "orders")
                .unwrap()
                .note
                .as_deref(),
            Some("from a"),
            "and back again"
        );

        forget();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that is not there is an error naming it, not an empty
    /// dictionary — a dictionary that silently became empty would look like a
    /// schema nobody had described.
    #[test]
    fn a_missing_file_says_which_one() {
        forget();
        let e = load(Path::new("/nonexistent/db-query/dictionary.toml")).expect_err("must fail");
        assert!(e.contains("dictionary at /nonexistent"), "{e}");
    }

    // ----------------------------------------- Stage 25 slice 5: the merge

    /// One table's worth of notes, as `put_notes` sends them.
    fn edit(
        table: &str,
        note: Option<&str>,
        join: Option<&str>,
        columns: &[(&str, &str)],
    ) -> NoteEdit {
        NoteEdit {
            table: table.into(),
            note: note.map(str::to_string),
            join: join.map(str::to_string),
            columns: columns
                .iter()
                .map(|(c, v)| ((*c).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn merged(text: &str, edits: &[NoteEdit], replace: bool) -> Merged {
        merge(text, "warehouse", edits, replace).expect("the file parses, so the merge runs")
    }

    /// The plain case: a table nobody has mentioned gets a section, and the
    /// dictionary says what was asked **when read back through the parser** —
    /// which is the only assertion worth making about generated TOML.
    #[test]
    fn a_table_the_file_never_mentioned_gets_a_section() {
        let out = merged(
            SAMPLE,
            &[edit(
                "shipments",
                Some("One row per parcel that left the building."),
                Some("shipments.order_id = orders.id"),
                &[("carrier", "DHL, UPS or ROYAL. Never null after dispatch.")],
            )],
            false,
        );
        assert_eq!(out.skipped, vec![]);
        assert_ne!(out.text, SAMPLE, "the text changed");
        assert_eq!(
            out.applied[0].wrote,
            vec!["note", "join", "columns.carrier"],
            "in spending order: what it is, how it joins, then its columns"
        );

        let d = parse(&out.text).expect("the result parses").dictionary;
        let said = d.table("warehouse", "shipments").expect("described now");
        assert_eq!(
            said.note.as_deref(),
            Some("One row per parcel that left the building.")
        );
        assert_eq!(said.join.as_deref(), Some("shipments.order_id = orders.id"));
        assert_eq!(
            d.column("warehouse", "shipments", "carrier").as_deref(),
            Some("DHL, UPS or ROYAL. Never null after dispatch.")
        );

        // And nothing that was already there moved.
        assert!(d.describes("warehouse", "orders"));
        assert_eq!(
            d.column("warehouse", "orders", "status").as_deref(),
            Some("P=pending, S=shipped, X=cancelled.")
        );
    }

    /// The stub case, which is what the generated starter leaves behind: the
    /// key is there and empty, so it is **filled**, not duplicated. A second
    /// `note` key in one section is a TOML error, so getting this wrong would
    /// break the file rather than look untidy.
    #[test]
    fn an_empty_note_in_an_existing_section_is_filled_in_place() {
        let text = "[tables.orders]\nnote = \"\"\ncolumns.status = \"\"\n";
        let out = merged(
            text,
            &[edit(
                "orders",
                Some("One row per order."),
                None,
                &[("status", "P, S or X.")],
            )],
            false,
        );
        assert_eq!(out.text.matches("note = ").count(), 1, "{}", out.text);
        assert_eq!(
            out.text.matches("columns.status = ").count(),
            1,
            "{}",
            out.text
        );
        let d = parse(&out.text).expect("parses").dictionary;
        assert_eq!(
            d.table("warehouse", "orders").unwrap().note.as_deref(),
            Some("One row per order.")
        );
        assert_eq!(
            d.column("warehouse", "orders", "status").as_deref(),
            Some("P, S or X.")
        );
    }

    /// **The safeguard.** A sentence somebody wrote by hand outranks a
    /// generated one: it is kept, and the caller is told it was kept rather
    /// than left to assume it was written.
    #[test]
    fn a_note_somebody_wrote_is_kept_unless_replacing_is_asked_for() {
        let edits = [edit(
            "orders",
            Some("A guess at what orders means."),
            None,
            &[],
        )];

        let out = merged(SAMPLE, &edits, false);
        assert_eq!(out.applied[0].kept, vec!["note"]);
        assert!(out.applied[0].wrote.is_empty());
        assert_eq!(out.text, SAMPLE, "the text is untouched byte for byte");

        let out = merged(SAMPLE, &edits, true);
        assert_eq!(out.applied[0].wrote, vec!["note"]);
        assert!(out.applied[0].kept.is_empty());
        let d = parse(&out.text).expect("parses").dictionary;
        assert_eq!(
            d.table("warehouse", "orders").unwrap().note.as_deref(),
            Some("A guess at what orders means.")
        );
        // The rest of that section survived the replacement.
        assert_eq!(
            d.table("warehouse", "orders").unwrap().join.as_deref(),
            Some("orders.customer_id = customers.id")
        );
    }

    /// A column note goes into **its own** table's section. The section is
    /// found by its header rather than by order, so the one risk worth a test
    /// is a note landing in the section above or below.
    #[test]
    fn a_column_note_lands_in_its_own_section() {
        let out = merged(
            SAMPLE,
            &[edit(
                "order_lines",
                None,
                None,
                &[("qty", "Base units, never packs.")],
            )],
            false,
        );
        let d = parse(&out.text).expect("parses").dictionary;
        assert_eq!(
            d.column("warehouse", "order_lines", "qty").as_deref(),
            Some("Base units, never packs.")
        );
        assert_eq!(d.column("warehouse", "orders", "qty"), None);
        // The section it was already in keeps its own note.
        assert!(d.table("warehouse", "order_lines").unwrap().note.is_some());
    }

    /// **Why `stub_block` exists.** `starter()` writes one commented-out
    /// section per real table — 385 of them in the reported setup — so
    /// appending live sections underneath would leave the file saying
    /// everything twice with the dead copy on top. The stub is replaced where
    /// it stands, and every other table's stub is left alone.
    #[test]
    fn a_generated_stub_is_replaced_where_it_stands() {
        let text = starter("warehouse", &["orders".into(), "order_lines".into()]);
        let out = merged(
            &text,
            &[edit("orders", Some("One row per order."), None, &[])],
            false,
        );

        assert!(
            !out.text.contains("# [tables.orders]"),
            "the stub it filled is gone: {}",
            out.text
        );
        assert!(
            out.text.contains("# [tables.order_lines]"),
            "the other table's stub is untouched: {}",
            out.text
        );
        assert!(
            out.text
                .contains("# What the tables and columns of `warehouse` mean."),
            "the file's own header comment survives: {}",
            out.text
        );
        assert_eq!(
            out.text.matches("[tables.orders]").count(),
            1,
            "{}",
            out.text
        );

        let d = parse(&out.text).expect("parses").dictionary;
        assert_eq!(
            d.table("warehouse", "orders").unwrap().note.as_deref(),
            Some("One row per order.")
        );
    }

    /// Prose is prose: quotation marks, a backslash, a newline, a tab and a
    /// control character that a basic string may not hold literally. Asserted
    /// by reading it back rather than by eye, because what matters is what the
    /// parser makes of it.
    #[test]
    fn prose_with_quotes_and_newlines_survives() {
        let prose = "Status is \"P\" or \"S\".\nPath: C:\\tmp\tand a bell: \u{7}";
        let out = merged("", &[edit("orders", Some(prose), None, &[])], false);
        let d = parse(&out.text).expect("the result parses").dictionary;
        assert_eq!(
            d.table("warehouse", "orders").unwrap().note.as_deref(),
            Some(prose)
        );
    }

    /// A form this module does not edit is **refused**, not papered over. The
    /// write would make the file unreadable — two definitions of one table —
    /// so it is reverted and the reason names the shape.
    #[test]
    fn a_columns_subtable_is_refused_rather_than_corrupted() {
        let text = "[tables.orders]\nnote = \"One row per order.\"\n\n[tables.orders.columns]\nstatus = \"P, S or X.\"\n";
        let out = merged(
            text,
            &[edit("orders", None, None, &[("total", "Cents.")])],
            false,
        );

        assert_eq!(out.text, text, "nothing was written");
        assert!(out.applied.is_empty());
        assert_eq!(out.skipped.len(), 1);
        assert!(
            out.skipped[0].why.contains("subtable"),
            "the reason names the shape: {}",
            out.skipped[0].why
        );
        // And the file still reads the way it did.
        let d = parse(&out.text).expect("parses").dictionary;
        assert_eq!(
            d.column("warehouse", "orders", "status").as_deref(),
            Some("P, S or X.")
        );
    }

    /// A per-database override decides this entry, so writing the shared one
    /// would be a write that **reads back differently** — reported rather than
    /// claimed. The readback is the only thing that catches this.
    #[test]
    fn an_override_that_shadows_the_write_is_reported() {
        let text = "[tables.orders]\nnote = \"Shared.\"\n\n[databases.warehouse.tables.orders]\nnote = \"Frozen copy.\"\n";
        let out = merged(
            text,
            &[edit("orders", Some("A better sentence."), None, &[])],
            true,
        );

        assert_eq!(out.text, text, "nothing was written");
        assert_eq!(out.skipped.len(), 1);
        assert!(
            out.skipped[0].why.contains("databases"),
            "the reason names what decides it: {}",
            out.skipped[0].why
        );
    }

    /// A file somebody is halfway through editing cannot be merged into:
    /// every decision above asks what it currently says.
    #[test]
    fn a_file_that_does_not_parse_is_refused_whole() {
        let err = merge(
            "[tables.orders\nnote = \"x\"",
            "warehouse",
            &[edit("orders", Some("x"), None, &[])],
            false,
        )
        .expect_err("a broken file is an error, not a merge");
        assert!(err.contains("cannot be read"), "{err}");
    }

    /// Nothing to say is said out loud. An agent that sent empty strings has a
    /// bug, and a silent success would hide it.
    #[test]
    fn an_edit_with_nothing_in_it_is_skipped_with_a_reason() {
        let out = merged(
            SAMPLE,
            &[edit("orders", Some("   "), Some(""), &[("status", " ")])],
            false,
        );
        assert!(out.applied.is_empty());
        assert_eq!(out.skipped.len(), 1);
        assert!(
            out.skipped[0].why.contains("empty"),
            "{}",
            out.skipped[0].why
        );
        assert_eq!(out.text, SAMPLE);
    }

    /// A note the length of an essay is refused: the whole annotation budget
    /// for one prompt is 8,000 characters (§2).
    #[test]
    fn a_note_longer_than_the_limit_is_refused() {
        let long = "x".repeat(MAX_NOTE_CHARS + 1);
        let out = merged(SAMPLE, &[edit("shipments", Some(&long), None, &[])], false);
        assert!(out.applied.is_empty());
        assert!(
            out.skipped[0].why.contains(&MAX_NOTE_CHARS.to_string()),
            "{}",
            out.skipped[0].why
        );
        assert_eq!(out.text, SAMPLE);
    }

    /// One table's failure is one table's failure. The others are still
    /// written, which is what makes a twenty-table call worth making.
    #[test]
    fn one_bad_table_does_not_cost_the_others() {
        let out = merged(
            SAMPLE,
            &[
                edit("", Some("nameless"), None, &[]),
                edit("shipments", Some("One row per parcel."), None, &[]),
            ],
            false,
        );
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.applied.len(), 1);
        assert_eq!(out.applied[0].table, "shipments");
        let d = parse(&out.text).expect("parses").dictionary;
        assert!(d.describes("warehouse", "shipments"));
    }

    /// The case somebody types is not the case a lookup needs, and both are
    /// true here: `ORDERS` finds the `[tables.orders]` that is already in the
    /// file rather than adding a second section.
    #[test]
    fn the_case_of_the_name_does_not_decide_which_section_is_written() {
        let out = merged(
            SAMPLE,
            &[edit("  ORDERS  ", None, None, &[("TOTAL", "Cents.")])],
            false,
        );
        assert_eq!(
            out.text.matches("[tables.orders]").count(),
            1,
            "{}",
            out.text
        );
        assert!(!out.text.contains("[tables.ORDERS]"), "{}", out.text);
        let d = parse(&out.text).expect("parses").dictionary;
        assert_eq!(
            d.column("warehouse", "orders", "total").as_deref(),
            Some("Cents.")
        );
    }
}
