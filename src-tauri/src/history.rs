//! What was run, so it can be found again.
//!
//! # Why JSON Lines rather than one JSON document
//!
//! History only ever appends, and it grows without limit. `profiles.rs` and
//! `workspace.rs` rewrite their whole file on every change, which is right for
//! a list of servers and wrong for a log: rewriting a few thousand entries
//! after every query would make running SQL slower the longer you used the app.
//!
//! One JSON object per line gives an O(1) append, and it fails *partially* — a
//! line truncated by a crash costs that line, not the file. A single JSON array
//! truncated mid-write costs everything.
//!
//! # Credentials are never recorded
//!
//! `CREATE USER … IDENTIFIED BY 'x'` puts a password in the SQL text, and this
//! project's rule is that secrets live in the keychain and nowhere else. Those
//! statements are **skipped entirely** rather than redacted: a redaction that
//! is subtly wrong writes the secret down anyway, and failing closed is the
//! only direction worth failing in. The history dialog says so, so an absence
//! is explained rather than mysterious.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "history.jsonl";

/// Entries kept after a compaction.
const KEEP_ENTRIES: usize = 5_000;
/// Compact once the file passes this. Checked with a metadata call rather than
/// by counting lines, so the common append stays O(1).
const COMPACT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Unix milliseconds. Stored rather than derived so the list can be ordered
    /// without trusting file order after a compaction.
    pub at: u64,
    pub connection_id: String,
    pub database: Option<String>,
    pub sql: String,
    pub kind: String,
    /// "ok" or "error" — a failed query is often the one you want back.
    pub status: String,
    #[serde(default)]
    pub rows: Option<u64>,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
    /// "user" or "assistant" — where the statement came from.
    ///
    /// Defaulted so history written before the assistant existed still reads,
    /// and so the *absence* of provenance means "yours", which is the safe
    /// direction: nothing gets attributed to the assistant by accident.
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "user".into()
}

/// One row of the history list: the newest run of a given statement, plus how
/// many times it has been run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    #[serde(flatten)]
    pub entry: Entry,
    /// How many recorded executions share this exact SQL.
    pub runs: usize,
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Does this statement carry a credential?
///
/// Deliberately narrow. Matching the bare word "password" would drop every
/// query against a `password_hash` column — a rule that silently eats ordinary
/// history is worse than one that occasionally misses. These are the actual
/// MySQL syntaxes that embed a secret in the statement text.
pub fn carries_credential(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    // `identified by` / `identified with … by` cover CREATE USER, ALTER USER
    // and GRANT; `set password` covers the rest.
    lower.contains("identified by")
        || lower.contains("identified with")
        || lower.contains("set password")
        || lower.contains("password(")
}

/// Append entries. Never returns an error to the caller's caller: failing to
/// remember a query must not fail the query.
pub fn record(dir: &Path, entries: &[Entry]) -> Result<(), String> {
    let recordable: Vec<&Entry> = entries
        .iter()
        .filter(|e| !carries_credential(&e.sql))
        .collect();
    if recordable.is_empty() {
        return Ok(());
    }

    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the config directory: {e}"))?;
    let path = path_in(dir);

    // Create and restrict before any content reaches it.
    if !path.exists() {
        fs::File::create(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
        restrict(&path).map_err(|e| format!("Cannot restrict {}: {e}", path.display()))?;
    }

    let mut buf = String::new();
    for e in recordable {
        let line =
            serde_json::to_string(e).map_err(|e| format!("Cannot serialise history: {e}"))?;
        buf.push_str(&line);
        buf.push('\n');
    }

    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    file.write_all(buf.as_bytes())
        .map_err(|e| format!("Cannot write history: {e}"))?;

    if file.metadata().map(|m| m.len()).unwrap_or(0) > COMPACT_BYTES {
        drop(file);
        compact(dir)?;
    }
    Ok(())
}

/// Every entry, oldest first. A line that will not parse is skipped rather than
/// failing the read — which is the whole reason for the line-per-entry format.
pub fn load(dir: &Path) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(path_in(dir)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        .collect()
}

/// Rewrite the file keeping only the newest [`KEEP_ENTRIES`].
fn compact(dir: &Path) -> Result<(), String> {
    let mut entries = load(dir);
    if entries.len() <= KEEP_ENTRIES {
        return Ok(());
    }
    entries.sort_by_key(|e| e.at);
    let keep = &entries[entries.len() - KEEP_ENTRIES..];

    let mut buf = String::new();
    for e in keep {
        buf.push_str(&serde_json::to_string(e).map_err(|e| format!("{e}"))?);
        buf.push('\n');
    }
    let path = path_in(dir);
    crate::files::write_atomic(&path, buf.as_bytes())?;
    restrict(&path).map_err(|e| format!("Cannot restrict {}: {e}", path.display()))?;
    Ok(())
}

/// Newest first, deduplicated by exact SQL text.
///
/// Running the same query twenty times should be one row that says twenty, not
/// twenty rows — the list exists to find a statement again, and repetition is
/// what pushes the interesting ones off the bottom.
pub fn search(
    dir: &Path,
    query: Option<&str>,
    connection_id: Option<&str>,
    limit: usize,
) -> Vec<Hit> {
    let needle = query
        .map(|q| q.trim().to_ascii_lowercase())
        .filter(|q| !q.is_empty());

    let mut entries = load(dir);
    entries.sort_by_key(|e| e.at);

    let mut hits: Vec<Hit> = Vec::new();
    // Newest first, so the first time a statement is seen is its latest run.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for e in entries.into_iter().rev() {
        if let Some(id) = connection_id {
            if e.connection_id != id {
                continue;
            }
        }
        if let Some(n) = &needle {
            if !e.sql.to_ascii_lowercase().contains(n.as_str()) {
                continue;
            }
        }
        match seen.get(&e.sql) {
            Some(&at) => hits[at].runs += 1,
            None => {
                seen.insert(e.sql.clone(), hits.len());
                hits.push(Hit { entry: e, runs: 1 });
            }
        }
    }
    hits.truncate(limit);
    hits
}

/// Forget everything, or everything for one connection.
pub fn clear(dir: &Path, connection_id: Option<&str>) -> Result<(), String> {
    let path = path_in(dir);
    let Some(id) = connection_id else {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("Cannot clear history: {e}"))?;
        }
        return Ok(());
    };

    let kept: Vec<Entry> = load(dir)
        .into_iter()
        .filter(|e| e.connection_id != id)
        .collect();
    let mut buf = String::new();
    for e in &kept {
        buf.push_str(&serde_json::to_string(e).map_err(|e| format!("{e}"))?);
        buf.push('\n');
    }
    crate::files::write_atomic(&path, buf.as_bytes())?;
    restrict(&path).map_err(|e| format!("Cannot restrict {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("db-query-hist-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(at: u64, sql: &str) -> Entry {
        Entry {
            at,
            connection_id: "c1".into(),
            database: Some("poc".into()),
            sql: sql.into(),
            kind: "select".into(),
            status: "ok".into(),
            rows: Some(1),
            elapsed_ms: 3,
            error: None,
            source: "user".into(),
        }
    }

    #[test]
    fn nothing_recorded_reads_as_empty() {
        assert!(search(&tmp(), None, None, 50).is_empty());
    }

    #[test]
    fn entries_come_back_newest_first() {
        let dir = tmp();
        record(&dir, &[entry(1, "SELECT 1"), entry(2, "SELECT 2")]).unwrap();
        let hits = search(&dir, None, None, 50);
        assert_eq!(hits[0].entry.sql, "SELECT 2");
        assert_eq!(hits[1].entry.sql, "SELECT 1");
    }

    /// Twenty runs of one query is one row that says twenty — otherwise a loop
    /// of the same statement buries everything else.
    #[test]
    fn repeated_statements_collapse_and_count() {
        let dir = tmp();
        record(
            &dir,
            &[
                entry(1, "SELECT 1"),
                entry(2, "SELECT 2"),
                entry(3, "SELECT 1"),
            ],
        )
        .unwrap();

        let hits = search(&dir, None, None, 50);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].entry.sql, "SELECT 1");
        assert_eq!(hits[0].runs, 2, "both runs counted");
        // And it keeps the *newest* occurrence, not the first.
        assert_eq!(hits[0].entry.at, 3);
    }

    #[test]
    fn search_is_case_insensitive_and_matches_anywhere() {
        let dir = tmp();
        record(
            &dir,
            &[entry(1, "SELECT * FROM Orders"), entry(2, "SHOW TABLES")],
        )
        .unwrap();
        let hits = search(&dir, Some("orders"), None, 50);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.sql, "SELECT * FROM Orders");
    }

    #[test]
    fn a_search_can_be_scoped_to_one_connection() {
        let dir = tmp();
        let mut other = entry(2, "SELECT 2");
        other.connection_id = "c2".into();
        record(&dir, &[entry(1, "SELECT 1"), other]).unwrap();

        let hits = search(&dir, None, Some("c1"), 50);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.sql, "SELECT 1");
    }

    /// The rule that matters most: a password must never reach the disk.
    #[test]
    fn statements_carrying_a_credential_are_never_recorded() {
        let dir = tmp();
        record(
            &dir,
            &[
                entry(1, "CREATE USER 'a'@'%' IDENTIFIED BY 'hunter2'"),
                entry(
                    2,
                    "ALTER USER 'a'@'%' IDENTIFIED WITH caching_sha2_password BY 's3cret'",
                ),
                entry(3, "SET PASSWORD FOR 'a'@'%' = 'p'"),
                entry(4, "SELECT PASSWORD('p')"),
                entry(5, "SELECT 1"),
            ],
        )
        .unwrap();

        let hits = search(&dir, None, None, 50);
        assert_eq!(hits.len(), 1, "only the innocent statement is kept");
        assert_eq!(hits[0].entry.sql, "SELECT 1");

        // And the secret is not anywhere in the file, not merely filtered on read.
        let raw = fs::read_to_string(path_in(&dir)).unwrap();
        assert!(!raw.contains("hunter2"));
        assert!(!raw.contains("s3cret"));
    }

    /// The rule is narrow on purpose: a `password_hash` column is ordinary SQL.
    #[test]
    fn an_ordinary_query_mentioning_passwords_is_still_recorded() {
        let dir = tmp();
        record(
            &dir,
            &[entry(1, "SELECT password_hash FROM users WHERE id = 1")],
        )
        .unwrap();
        assert_eq!(search(&dir, None, None, 50).len(), 1);
    }

    /// The reason for one-object-per-line: a torn write costs one entry.
    #[test]
    fn a_corrupt_line_costs_only_itself() {
        let dir = tmp();
        record(&dir, &[entry(1, "SELECT 1")]).unwrap();
        let mut f = OpenOptions::new().append(true).open(path_in(&dir)).unwrap();
        f.write_all(b"{\"at\": 2, truncated...\n").unwrap();
        drop(f);
        record(&dir, &[entry(3, "SELECT 3")]).unwrap();

        let hits = search(&dir, None, None, 50);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].entry.sql, "SELECT 3");
    }

    #[test]
    fn clearing_one_connection_leaves_the_others() {
        let dir = tmp();
        let mut other = entry(2, "SELECT 2");
        other.connection_id = "c2".into();
        record(&dir, &[entry(1, "SELECT 1"), other]).unwrap();

        clear(&dir, Some("c1")).unwrap();
        let hits = search(&dir, None, None, 50);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.connection_id, "c2");
    }

    #[test]
    fn clearing_everything_removes_the_file() {
        let dir = tmp();
        record(&dir, &[entry(1, "SELECT 1")]).unwrap();
        clear(&dir, None).unwrap();
        assert!(!path_in(&dir).exists());
        assert!(search(&dir, None, None, 50).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp();
        record(&dir, &[entry(1, "SELECT 1")]).unwrap();
        let mode = fs::metadata(path_in(&dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn compaction_keeps_the_newest_entries() {
        let dir = tmp();
        // Unique SQL per entry, so nothing collapses and the count is the count.
        let many: Vec<Entry> = (0..KEEP_ENTRIES as u64 + 10)
            .map(|i| entry(i + 1, &format!("SELECT {i}")))
            .collect();
        record(&dir, &many).unwrap();
        compact(&dir).unwrap();

        let kept = load(&dir);
        assert_eq!(kept.len(), KEEP_ENTRIES);
        assert_eq!(kept.last().unwrap().at, KEEP_ENTRIES as u64 + 10);
        assert!(kept.iter().all(|e| e.at > 10), "the oldest were dropped");
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::*;

    /// History written before the assistant existed has no `source`. It must
    /// still read, and must read as the user's — never as the assistant's.
    #[test]
    fn an_entry_with_no_source_is_the_users() {
        let line = r#"{"at":1,"connectionId":"c1","database":null,"sql":"SELECT 1",
            "kind":"select","status":"ok","rows":1,"elapsedMs":2}"#;
        let e: Entry = serde_json::from_str(&line.replace('\n', "")).unwrap();
        assert_eq!(e.source, "user");
    }
}
