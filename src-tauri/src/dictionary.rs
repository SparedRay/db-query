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
    /// `tables` is every table name in the database; `columns` answers which
    /// columns a table has, and returns `None` for a table whose columns have
    /// not been loaded — in which case its column notes are **not** reported,
    /// because "not loaded yet" is not "not there".
    pub fn unmatched<'a, F>(&self, database: &str, tables: &[String], columns: F) -> Vec<Unmatched>
    where
        F: Fn(&str) -> Option<&'a [String]>,
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
            let Some(have) = columns(name) else { continue };
            let have: std::collections::BTreeSet<String> = have.iter().map(|c| fold(c)).collect();
            for col in entry.columns.keys() {
                if !have.contains(col) {
                    out.push(Unmatched {
                        table: name.clone(),
                        column: Some(col.clone()),
                    });
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
        let orders_cols = [
            "id".to_string(),
            "customer_id".to_string(),
            "status".to_string(),
        ];

        let found = d.unmatched("warehouse", &tables, |t| match t {
            "orders" => Some(&orders_cols),
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
        let found = d.unmatched("warehouse", &tables, |_| None);
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
            d.unmatched("archive", &tables, |_| None)
                .iter()
                .map(Unmatched::label)
                .collect::<Vec<_>>(),
            ["oders"]
        );
        // And it is not reported against a database it does not apply to.
        assert!(d.unmatched("live", &tables, |_| None).is_empty());
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
}
