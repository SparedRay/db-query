//! Script execution: classify, rewrite, run sequentially, report honestly.

use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use sqlx::Executor;

use crate::decode::{columns_of, decode_cell, CellValue, ColumnMeta};
use crate::session::{self, friendly, AppState, TabSession};
use crate::split;

pub const MAX_ROWS: usize = 5000;

/// Ceiling on rows held for one script, across all of its statements.
///
/// `MAX_ROWS` is per statement, so a ten-SELECT script could hold 50,000 rows —
/// and with several tabs open that multiplies again. This bounds one tab's
/// worst case regardless of how many statements it runs.
pub const MAX_SCRIPT_ROWS: usize = 20_000;

/// Wrap user SQL for sqlx 0.9's `SqlSafeStr` bound.
///
/// That bound exists as a speed bump against `format!`-ing user input into a
/// query. Here the dynamic string *is* the product: this is a SQL client, and
/// the text the user typed in the editor is precisely what we are asked to
/// send. There is no injection boundary to protect — the user already has a
/// direct SQL channel, limited by their own MySQL grants. Asserting is correct.
///
/// This is NOT a licence to interpolate elsewhere. Identifiers we build
/// ourselves (`USE`, schema lookups) are quoted or bound; see `session.rs`.
fn raw(sql: String) -> sqlx::RawSql {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatementKind {
    /// Eligible for auto-LIMIT.
    Select,
    /// Returns rows but must never be rewritten: SHOW / DESCRIBE / EXPLAIN.
    RowReturning,
    Modify,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Outcome {
    Rows {
        columns: Vec<ColumnMeta>,
        rows: Vec<Vec<CellValue>>,
        truncated: bool,
    },
    Affected {
        rows: u64,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub sql: String,
    /// `Some` only when we rewrote the statement. The UI must surface this.
    pub effective_sql: Option<String>,
    pub kind: StatementKind,
    pub outcome: Outcome,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptResult {
    pub statements: Vec<StatementResult>,
    pub total_elapsed_ms: u64,
    pub aborted_at: Option<usize>,
    pub delimiter_detected: bool,
    pub cancelled: bool,
    /// The safety-net timeout fired. Distinct from `cancelled`, which means
    /// the user pressed the button.
    pub timed_out: bool,
}

/// Await a query, optionally KILLing it after `timeout_secs`.
///
/// On expiry we do NOT drop the future — that would abandon a connection
/// mid-protocol and leave it unusable for the rest of the session. Instead we
/// KILL from the idle control connection and then let the original future
/// unwind normally, which is what keeps the exec connection reusable.
async fn await_with_timeout<F, T>(
    fut: F,
    timeout_secs: Option<u64>,
    state: &AppState,
    tab_id: &str,
    timed_out: &mut bool,
) -> Result<T, sqlx::Error>
where
    F: Future<Output = Result<T, sqlx::Error>>,
{
    tokio::pin!(fut);

    let mut expired = false;
    let early = match timeout_secs.filter(|s| *s > 0) {
        Some(secs) => tokio::select! {
            r = &mut fut => Some(r),
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                expired = true;
                None
            }
        },
        None => None,
    };

    if let Some(r) = early {
        return r;
    }
    if expired {
        *timed_out = true;
        // Also sets this tab's cancel_requested, so the rest of its script is
        // abandoned. Other tabs are untouched.
        session::cancel_query(state, tab_id).await.ok();
    }
    fut.await
}

/// First bare word of the statement, uppercased, read from the masked text so
/// a leading comment or a quoted identifier cannot fool it.
fn leading_keyword(masked: &str) -> String {
    masked
        .trim_start_matches(|c: char| c.is_whitespace() || c == '(')
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

pub fn classify(sql: &str) -> StatementKind {
    let masked = split::mask_noncode(sql);
    match leading_keyword(&masked).as_str() {
        "SELECT" | "WITH" | "TABLE" | "VALUES" => StatementKind::Select,
        // These return rows too. Dispatching on "starts with SELECT" would
        // silently drop their output.
        "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "ANALYZE" | "CHECK" | "CHECKSUM" | "HELP" => {
            StatementKind::RowReturning
        }
        "INSERT" | "UPDATE" | "DELETE" | "REPLACE" => StatementKind::Modify,
        _ => StatementKind::Other,
    }
}

pub fn returns_rows(kind: StatementKind) -> bool {
    matches!(kind, StatementKind::Select | StatementKind::RowReturning)
}

/// Find a whole-word keyword at paren depth 0 in already-masked text.
fn has_top_level_word(masked: &str, word: &str) -> bool {
    let up = masked.to_ascii_uppercase();
    let bytes = up.as_bytes();
    let w = word.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {
                if depth == 0 && bytes[i..].starts_with(w) {
                    let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
                    let after = bytes.get(i + w.len());
                    let after_ok = after.is_none_or(|&c| !is_ident_byte(c));
                    if before_ok && after_ok {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Clauses that must follow LIMIT in MySQL grammar. Appending `LIMIT 5000`
/// after them produces a syntax error, so we leave those statements alone
/// rather than sending SQL that cannot parse.
fn blocks_auto_limit(masked: &str) -> bool {
    has_top_level_word(masked, "FOR")
        || has_top_level_word(masked, "LOCK")
        || has_top_level_word(masked, "INTO")
        || has_top_level_word(masked, "PROCEDURE")
}

/// Returns the rewritten SQL when auto-LIMIT applies, else `None`.
pub fn auto_limit(sql: &str, kind: StatementKind, enabled: bool) -> Option<String> {
    if !enabled || kind != StatementKind::Select {
        return None;
    }
    let masked = split::mask_noncode(sql);
    if has_top_level_word(&masked, "LIMIT") || blocks_auto_limit(&masked) {
        return None;
    }
    Some(format!(
        "{} LIMIT {MAX_ROWS}",
        sql.trim_end_matches(';').trim_end()
    ))
}

/// Does this statement switch the active database? Keeps the sidebar honest
/// when a script contains a bare `USE somedb;`.
fn used_database(sql: &str) -> Option<String> {
    let masked = split::mask_noncode(sql);
    if leading_keyword(&masked) != "USE" {
        return None;
    }
    // Read the name from the ORIGINAL text so a backticked name survives.
    let rest = sql.trim_start();
    let rest = rest.get(3..)?.trim();
    let name = rest.trim_end_matches(';').trim();
    let name = name
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .map(|s| s.replace("``", "`"))
        .unwrap_or_else(|| name.to_string());
    (!name.is_empty()).then_some(name)
}

pub async fn run_script(
    state: &AppState,
    tab_id: &str,
    sql: &str,
    auto_limit_enabled: bool,
    timeout_secs: Option<u64>,
) -> Result<ScriptResult, String> {
    let split_out = split::split(sql);
    // A DELIMITER block is handed to the server verbatim: no rewriting, no
    // per-statement tabs, because the terminator is no longer `;`.
    let auto_limit_enabled = auto_limit_enabled && !split_out.delimiter_detected;

    // Clone the Arc out and release the map lock before doing any I/O. Holding
    // it across the query would serialize every tab through one mutex and undo
    // the whole point of per-tab connections. The tab carries its own server,
    // so there is no way to run this against the wrong one.
    let tab: Arc<TabSession> = session::tab(state, tab_id).await?;

    let mut guard = tab.exec.lock().await;
    session::ensure_exec(&mut guard, &tab).await?;
    let conn = guard
        .as_mut()
        .expect("ensure_exec guarantees a live connection");

    tab.cancel_requested.store(false, Ordering::SeqCst);
    tab.running.store(true, Ordering::SeqCst);

    let script_start = Instant::now();
    let mut statements = Vec::new();
    let mut aborted_at = None;
    let mut cancelled = false;
    let mut timed_out = false;
    let mut new_db: Option<String> = None;
    let mut rows_budget = MAX_SCRIPT_ROWS;

    for (idx, span) in split_out.statements.iter().enumerate() {
        if tab.cancel_requested.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }

        let text = &sql[span.start..span.end];
        let kind = if split_out.delimiter_detected {
            StatementKind::Other
        } else {
            classify(text)
        };
        let rewritten = auto_limit(text, kind, auto_limit_enabled);
        // sqlx 0.9 requires SQL to be `&'static str` or explicitly asserted
        // safe. An owned String is the zero-copy path through AssertSqlSafe.
        let to_run: String = rewritten.clone().unwrap_or_else(|| text.to_string());

        let started = Instant::now();
        let outcome = if returns_rows(kind) {
            let fut = (&mut *conn).fetch_all(raw(to_run));
            match await_with_timeout(fut, timeout_secs, state, tab_id, &mut timed_out).await {
                Ok(rows) => {
                    let columns = rows.first().map(columns_of).unwrap_or_default();
                    // Spend from the script-wide budget before decoding, so a
                    // long script cannot balloon memory one statement at a time.
                    let kept = rows.len().min(rows_budget);
                    let over_budget = kept < rows.len();
                    rows_budget -= kept;

                    let truncated = over_budget || (rewritten.is_some() && rows.len() >= MAX_ROWS);
                    let decoded = rows
                        .iter()
                        .take(kept)
                        .map(|r| (0..columns.len()).map(|i| decode_cell(r, i)).collect())
                        .collect();
                    Outcome::Rows {
                        columns,
                        rows: decoded,
                        truncated,
                    }
                }
                Err(e) => Outcome::Error {
                    message: friendly(&e),
                },
            }
        } else {
            let fut = (&mut *conn).execute(raw(to_run));
            match await_with_timeout(fut, timeout_secs, state, tab_id, &mut timed_out).await {
                Ok(r) => {
                    if let Some(db) = used_database(text) {
                        new_db = Some(db);
                    }
                    Outcome::Affected {
                        rows: r.rows_affected(),
                    }
                }
                Err(e) => Outcome::Error {
                    message: friendly(&e),
                },
            }
        };

        let is_error = matches!(outcome, Outcome::Error { .. });
        statements.push(StatementResult {
            sql: text.to_string(),
            effective_sql: rewritten,
            kind,
            outcome,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });

        // Stop at the first error, but keep everything that already succeeded.
        if is_error {
            aborted_at = Some(idx);
            if tab.cancel_requested.load(Ordering::SeqCst) {
                cancelled = true;
            }
            break;
        }
    }

    // A KILLed statement does not necessarily come back as an error: MySQL's
    // SLEEP() returns 1 when interrupted, so a cancelled single-statement
    // script would otherwise look like an ordinary success in the status bar.
    let cancelled = cancelled || tab.cancel_requested.load(Ordering::SeqCst);

    tab.running.store(false, Ordering::SeqCst);
    drop(guard);

    // A bare `USE somedb;` in the script moves THIS tab's active database, and
    // only this tab's — each script keeps its own.
    if let Some(db) = new_db {
        *tab.current_db.lock().await = Some(db);
    }

    Ok(ScriptResult {
        statements,
        total_elapsed_ms: script_start.elapsed().as_millis() as u64,
        aborted_at,
        delimiter_detected: split_out.delimiter_detected,
        cancelled,
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_row_returning_statements_separately_from_select() {
        assert_eq!(classify("SELECT 1"), StatementKind::Select);
        assert_eq!(classify("  select 1"), StatementKind::Select);
        assert_eq!(
            classify("WITH x AS (SELECT 1) SELECT * FROM x"),
            StatementKind::Select
        );
        assert_eq!(
            classify("(SELECT 1) UNION (SELECT 2)"),
            StatementKind::Select
        );
        assert_eq!(classify("SHOW TABLES"), StatementKind::RowReturning);
        assert_eq!(classify("DESCRIBE users"), StatementKind::RowReturning);
        assert_eq!(classify("EXPLAIN SELECT 1"), StatementKind::RowReturning);
        assert_eq!(classify("INSERT INTO t VALUES (1)"), StatementKind::Modify);
        assert_eq!(classify("CREATE TABLE t (a INT)"), StatementKind::Other);
        assert_eq!(classify("USE mydb"), StatementKind::Other);
    }

    #[test]
    fn leading_comment_does_not_hide_the_keyword() {
        assert_eq!(classify("-- note\nSELECT 1"), StatementKind::Select);
        assert_eq!(
            classify("/* note */ SHOW TABLES"),
            StatementKind::RowReturning
        );
    }

    #[test]
    fn auto_limit_appends_to_a_bare_select() {
        assert_eq!(
            auto_limit("SELECT * FROM users", StatementKind::Select, true).as_deref(),
            Some("SELECT * FROM users LIMIT 5000")
        );
    }

    #[test]
    fn auto_limit_respects_an_existing_top_level_limit() {
        assert!(auto_limit("SELECT * FROM t LIMIT 10", StatementKind::Select, true).is_none());
    }

    #[test]
    fn auto_limit_still_applies_when_limit_is_only_in_a_subquery() {
        // The inner LIMIT bounds the subquery, not the result set.
        assert_eq!(
            auto_limit(
                "SELECT * FROM (SELECT * FROM t LIMIT 10) x",
                StatementKind::Select,
                true
            )
            .as_deref(),
            Some("SELECT * FROM (SELECT * FROM t LIMIT 10) x LIMIT 5000")
        );
    }

    #[test]
    fn auto_limit_ignores_the_word_limit_in_strings_and_identifiers() {
        assert!(auto_limit("SELECT 'limit' FROM t", StatementKind::Select, true).is_some());
        assert!(auto_limit("SELECT `limit` FROM t", StatementKind::Select, true).is_some());
        assert!(auto_limit("SELECT 1 -- limit", StatementKind::Select, true).is_some());
    }

    #[test]
    fn auto_limit_never_touches_non_select_kinds() {
        assert!(auto_limit("SHOW TABLES", StatementKind::RowReturning, true).is_none());
        assert!(auto_limit("DELETE FROM t", StatementKind::Modify, true).is_none());
        assert!(auto_limit("SELECT * FROM t", StatementKind::Select, false).is_none());
    }

    #[test]
    fn auto_limit_backs_off_where_limit_would_be_a_syntax_error() {
        // LIMIT must precede FOR UPDATE / LOCK IN SHARE MODE / INTO OUTFILE.
        assert!(auto_limit("SELECT * FROM t FOR UPDATE", StatementKind::Select, true).is_none());
        assert!(auto_limit(
            "SELECT * FROM t LOCK IN SHARE MODE",
            StatementKind::Select,
            true
        )
        .is_none());
        assert!(auto_limit(
            "SELECT * FROM t INTO OUTFILE '/tmp/x'",
            StatementKind::Select,
            true
        )
        .is_none());
    }

    #[test]
    fn auto_limit_strips_a_trailing_semicolon_before_appending() {
        assert_eq!(
            auto_limit("SELECT 1;", StatementKind::Select, true).as_deref(),
            Some("SELECT 1 LIMIT 5000")
        );
    }

    #[test]
    fn detects_the_active_database_from_a_use_statement() {
        assert_eq!(used_database("USE shop"), Some("shop".into()));
        assert_eq!(used_database("use `my db`"), Some("my db".into()));
        assert_eq!(used_database("USE shop;"), Some("shop".into()));
        assert_eq!(used_database("SELECT 1"), None);
    }
}
