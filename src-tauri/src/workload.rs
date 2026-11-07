//! What the server says about its own workload.
//!
//! # Why a verdict and not a ranking
//!
//! The obvious design is "sort by slowest, offer `CREATE INDEX`". Measured
//! against a real server it is wrong: the second most expensive shape on the
//! instance this stage was built against reads 1.51 billion rows and
//! **returns all of them**, so no index can shrink it. An advisor that sorted
//! by time would have proposed one anyway, confidently, for the thing it
//! understood least.
//!
//! So every shape gets a [`Verdict`] — what *kind* of problem it is — and four
//! of the seven verdicts refuse to propose anything. See §4.2 of the Stage 31
//! doc.
//!
//! The refusals are evaluated **before** [`Verdict::MissingIndex`], and it is
//! worth being precise about where that ordering earns its keep, because it is
//! not where it looks. [`Verdict::NoIndexWillHelp`] and
//! [`Verdict::MissingIndex`] are separated by *disjoint ratio bands* and could
//! be checked in either order. The three that genuinely depend on running
//! first are [`Verdict::PlatformInternal`], [`Verdict::DriverChatter`] — a
//! `SHOW STATUS` scans a table 29 million times and finds no index, which is
//! the index proposal's trigger exactly — and
//! [`Verdict::ViewBlocksFilter`], where the worst ratio measured on a real
//! server belongs to a view that no index can be added to.
//!
//! # Why this module is almost all pure functions
//!
//! Everything from [`DigestRow`] onwards is a free function over borrowed
//! slices: no `ServerConn`, no SQL, no async. The rules are the part most
//! likely to be wrong, so they are the part that has to be testable without a
//! server — the shape `relevance.rs` took in Stage 30, for the same reason.
//! The tests at the bottom run against the real digest rows that were
//! measured, offline.
//!
//! # Where the literals went
//!
//! Nowhere: there are none. MySQL normalises every statement into a digest
//! with its values replaced by `?` before it ever reaches a counter, so
//! nothing in this module has ever seen a parameter. That is a property of the
//! source rather than a redaction, and it is what makes this safe to show, log
//! and keep. It is also why the slow query log is deliberately not read here —
//! `mysql.slow_log` holds the values, and that is a different argument.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::Row;

use crate::exec;
use crate::schema::DbSchema;
use crate::session::{friendly, ServerConn};
use crate::split;

/// How many shapes are sent to the frontend, most expensive first.
///
/// The digest table itself is read **whole**, because per-table write pressure
/// ([`write_pressure`]) is a total and a top-N would quietly make it a sample.
/// Only the shipping is bounded: 10,000 digests of up to a kilobyte each is a
/// reasonable thing to aggregate in Rust and an unreasonable thing to hand a
/// web view.
pub const SHAPES_SENT: usize = 200;

/// Rows examined per row returned, above which an index is worth proposing.
///
/// Deliberately blunt. The worst real shape measured sat at 2,599 and the
/// worst at 109,342,481, so the exact cut between 20 and 200 changes nothing
/// that matters; what matters is that it is stated here once rather than
/// implied in three places.
const RATIO_WANTS_INDEX: f64 = 100.0;

/// Rows examined per row returned, below which the query wants what it reads.
const RATIO_READS_WHAT_IT_RETURNS: f64 = 2.0;

/// The two ratio bands must stay disjoint.
///
/// Written down because the obvious reading of [`verdict_of`] is that
/// [`Verdict::NoIndexWillHelp`] sits above [`Verdict::MissingIndex`] in order
/// to win a race against it. It does not — no ratio satisfies both — so their
/// relative order is **not** load-bearing, and the checks that genuinely are
/// come earlier still. If somebody widens one band until they overlap, the
/// ordering silently starts to matter, and a build error is a better place to
/// find that out than a test run.
const _: () = assert!(RATIO_READS_WHAT_IT_RETURNS < RATIO_WANTS_INDEX);

/// Average microseconds above which "and it examined nothing" means waiting
/// rather than a trivially fast statement.
const WAITING_AVG_US: u64 = 50_000;

/// How close two execution counts must be to look like one loop.
const LOOP_TOLERANCE: f64 = 0.001;

/// How close two shapes' first sighting must be to look like one loop, in
/// microseconds.
const LOOP_FIRST_SEEN_US: u64 = 1_000_000;

/// Table names that mean the statement is not the application's.
///
/// **This is a list on purpose, and it is the only one in this module.** The
/// chatter rules in [`is_chatter`] are structural and will classify the next
/// driver's handshake without being taught it, because "sets session state"
/// and "asks the server about itself" have shapes. A managed platform's
/// internal housekeeping has no shape — only names — so naming them is the
/// honest implementation rather than a shortcut.
///
/// Matched as a prefix against the last segment of a referenced table.
const PLATFORM_TABLES: &[&str] = &[
    "rds_heartbeat",
    "rds_metrics_",
    "rds_configuration",
    "rds_history",
    "rds_replication_status",
    "rds_sysinfo",
    "replica_host_status",
    "oscar_",
    "aurora_",
    "innodb_metrics",
];

/// Keywords after which the next identifier is a table.
const BEFORE_A_TABLE: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE"];

/// One row of `performance_schema.events_statements_summary_by_digest`.
///
/// Times are **picoseconds**, which is what the table stores. They are kept
/// that way rather than converted on read so that nothing in here has to
/// remember which unit it is holding; the two accessors that convert say so in
/// their names.
#[derive(Debug, Clone, Default)]
pub struct DigestRow {
    /// `None` for a statement issued with no default database — which includes
    /// real application queries that fully qualify their tables, so it must
    /// never be used to decide whether a shape is the application's.
    pub schema: Option<String>,
    pub digest: String,
    pub text: String,
    pub execs: u64,
    pub total_picos: u64,
    pub avg_picos: u64,
    pub lock_picos: u64,
    pub errors: u64,
    pub rows_sent: u64,
    pub rows_examined: u64,
    pub no_index: u64,
    /// `SUM_SELECT_SCAN`: executions that scanned a table from the first row.
    pub scans: u64,
    pub sort_scans: u64,
    pub tmp_disk: u64,
    /// MySQL 8.0 only. Far better than the average for a shape that is usually
    /// fine and occasionally terrible.
    pub quantile_95_picos: Option<u64>,
    pub first_seen: String,
    pub last_seen: String,
    pub first_seen_us: u64,
    pub last_seen_us: u64,
}

impl DigestRow {
    pub fn total_secs(&self) -> f64 {
        self.total_picos as f64 / 1e12
    }

    pub fn avg_millis(&self) -> f64 {
        self.avg_picos as f64 / 1e9
    }

    fn avg_micros(&self) -> u64 {
        self.avg_picos / 1_000_000
    }

    /// Rows examined per row returned, or `None` when nothing was returned.
    ///
    /// `None` rather than zero or infinity: a statement that returns no rows
    /// has no ratio, and an `UPDATE` examining one row per execution is
    /// healthy. Treating that as a division by zero and calling it the worst
    /// shape on the server would be the easiest way to make this whole module
    /// untrustworthy.
    pub fn ratio(&self) -> Option<f64> {
        if self.rows_sent == 0 {
            None
        } else {
            Some(self.rows_examined as f64 / self.rows_sent as f64)
        }
    }
}

/// What kind of problem a shape is — the headline, and whether anything is
/// proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// Reads far more than it returns, and at least some executions found no
    /// index. The only verdict that proposes an index.
    MissingIndex,
    /// Reads far more than it returns, and would be [`Verdict::MissingIndex`]
    /// except that the filter wraps a column in a function call. No b-tree
    /// index can serve that, so this proposes nothing.
    ///
    /// **This verdict exists because the panel was wrong.** The most
    /// expensive shape on the UAT server -- 41M executions, 79h 24m, 2,599
    /// rows read per row returned -- was reported as a missing index on
    /// 2026-11-05, and its filter is
    /// `NAME = ? OR JSON_CONTAINS(LOWER(COALESCE(ALIAS, ?)), ...)`. Any index
    /// built on that advice would have gone unused, on the one statement
    /// where the cost of being wrong was highest.
    PredicateNotIndexable,
    /// Returns everything it reads. A full scan is not a mistake here.
    NoIndexWillHelp,
    /// The target is a view, so the predicate cannot reach the base tables.
    ViewBlocksFilter,
    /// Several shapes on one table, run the same number of times, started
    /// together. Assigned by [`loops`], not by [`classify`].
    RowByRowLoop,
    /// Spends its time blocked rather than reading.
    WaitingNotReading,
    /// Session setup and server self-inspection. Hidden by default.
    DriverChatter,
    /// The managed platform's own housekeeping. Always hidden.
    PlatformInternal,
    /// None of the rules fired. Said out loud rather than guessed at.
    Unclassified,
}

impl Verdict {
    /// Whether this verdict leads to a `CREATE INDEX`.
    pub fn proposes_an_index(self) -> bool {
        self == Verdict::MissingIndex
    }

    pub fn class(self) -> Class {
        match self {
            Verdict::DriverChatter => Class::Chatter,
            Verdict::PlatformInternal => Class::Platform,
            _ => Class::Application,
        }
    }
}

/// Which of the three lists a shape belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Class {
    Application,
    Chatter,
    Platform,
}

/// Every table name the connected server has, and whether it is a view.
///
/// Keyed on the **lowercased bare name**, not `db.table`, because a digest
/// need not name the database — `SCHEMA_NAME` is empty for anything issued
/// without a default database, and a fully qualified query inside a digest
/// gets its own prefix stripped by [`candidates`].
///
/// When one name exists in two databases and only one of them is a view, it is
/// recorded as a **view**. That direction is deliberate: calling a real table
/// a view costs a refused proposal, and calling a view a real table costs a
/// proposal for an index that cannot exist.
pub type KnownTables = HashMap<String, bool>;

/// Build the name index from whatever the schema cache has warmed.
pub fn known_tables<'a>(schemas: impl Iterator<Item = &'a DbSchema>) -> KnownTables {
    let mut known = KnownTables::new();
    for schema in schemas {
        for t in schema.tables.as_deref().unwrap_or_default() {
            let is_view = t.kind.eq_ignore_ascii_case("VIEW");
            known
                .entry(t.name.to_ascii_lowercase())
                .and_modify(|v| *v = *v || is_view)
                .or_insert(is_view);
        }
    }
    known
}

/// Every identifier that follows `FROM`, `JOIN`, `INTO` or `UPDATE`, as the
/// digest spells it, lowercased and reduced to its last segment.
///
/// **Not a SQL parser, and it does not pretend to be one.** A parser is larger
/// than the rest of this stage put together. The safety net is that a
/// candidate only becomes a reported table if the schema already has that name
/// ([`tables_named`]), so a bad read costs a shape its verdict rather than
/// inventing a finding about a table nobody has.
pub fn candidates(text: &str) -> Vec<String> {
    // `mask_keep_idents` and not `mask_noncode`: the latter blanks quoted
    // identifiers along with string literals, so `` FROM `Supplier` `` reads
    // as `FROM` and nothing else. `lint.rs` hit this first and the comment on
    // `mask_keep_idents` says so — "the linter needs this". Same question
    // here, same mask.
    let masked = split::mask_keep_idents(text);
    let tokens: Vec<&str> = masked
        .split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')' || c == ';')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        if !BEFORE_A_TABLE.contains(&tokens[i].to_ascii_uppercase().as_str()) {
            i += 1;
            continue;
        }

        // Walk the qualified name that follows. A digest writes it
        // `appdata` . `partysettings`, and once the mask has
        // turned the backticks into spaces the dot is a token of its own — so
        // "the token after FROM" is the *database*, and taking it would name
        // a table nobody has. Keep walking while dots keep coming; the table
        // is the last segment either way.
        let mut j = i + 1;
        let mut name: Option<&str> = None;
        while let Some(token) = tokens.get(j) {
            if *token == "." {
                j += 1;
                continue;
            }
            name = Some(token.rsplit('.').next().unwrap_or(token));
            if tokens.get(j + 1).is_some_and(|t| *t == ".") {
                j += 2;
                continue;
            }
            break;
        }

        if let Some(found) = name {
            let lower = found.trim_matches('.').to_ascii_lowercase();
            // `FROM ( SELECT …` is a derived table. Its own `FROM` is still
            // ahead of the cursor, so there is nothing to record here and
            // nothing to skip past.
            if !lower.is_empty() && lower != "select" && !out.contains(&lower) {
                out.push(lower);
            }
        }

        i = j.max(i + 1);
    }
    out
}

/// The candidates the schema actually has a name for.
///
/// The same guard as [`crate::relevance::recognised`], for the same reason: a
/// name this module reports is a name somebody will go looking for, and a
/// crude scan plus a hard filter cannot send them after a table that does not
/// exist.
pub fn tables_named(text: &str, known: &KnownTables) -> Vec<String> {
    candidates(text)
        .into_iter()
        .filter(|c| known.contains_key(c))
        .collect()
}

/// Whether any referenced name belongs to the managed platform.
fn is_platform(cands: &[String]) -> bool {
    cands
        .iter()
        .any(|c| PLATFORM_TABLES.iter().any(|p| c.starts_with(p)))
}

/// Whether this is session setup or the server being asked about itself.
///
/// Three structural rules, no list of statements:
///
/// * [`exec::StatementKind::Session`] — `SET`, `USE`, `COMMIT` and friends.
/// * a leading `SHOW`.
/// * a select list of nothing but `@@` variables, which is what a connection
///   pool sends to learn the server's settings. Recognised by *having no
///   `FROM`*, so a real query that happens to mention `@@sql_mode` beside a
///   table is not swept up with it.
fn is_chatter(text: &str) -> bool {
    if exec::classify(text) == exec::StatementKind::Session {
        return true;
    }
    let masked = split::mask_noncode(text);
    let leading = exec::leading_keyword_of(text);
    if leading == "SHOW" {
        return true;
    }
    if masked.contains("@@") && !has_from(&masked) {
        return true;
    }
    false
}

fn has_from(masked: &str) -> bool {
    masked
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|w| w.eq_ignore_ascii_case("from"))
}

/// One shape, classified.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub digest: String,
    pub text: String,
    pub database: Option<String>,
    pub verdict: Verdict,
    pub class: Class,
    /// Only names the schema has, so this is safe to show and to log.
    pub tables: Vec<String>,
    pub execs: u64,
    pub total_secs: f64,
    pub avg_millis: f64,
    pub p95_millis: Option<f64>,
    pub rows_sent: u64,
    pub rows_examined: u64,
    pub ratio: Option<f64>,
    pub no_index: u64,
    pub scans: u64,
    pub sort_scans: u64,
    pub tmp_disk: u64,
    pub errors: u64,
    pub lock_secs: f64,
    pub first_seen: String,
    pub last_seen: String,
}

/// The verdict for one shape, in the order §4.2 fixed.
///
/// The refusals come first. `MissingIndex` is the **last** rule that can fire
/// before `Unclassified`, so every shape that could be explained another way
/// already has been.
pub fn classify(row: &DigestRow, known: &KnownTables) -> Finding {
    let cands = candidates(&row.text);
    let tables: Vec<String> = cands
        .iter()
        .filter(|c| known.contains_key(*c))
        .cloned()
        .collect();

    let verdict = verdict_of(row, &cands, &tables, known);

    Finding {
        digest: row.digest.clone(),
        text: row.text.clone(),
        database: row.schema.clone(),
        verdict,
        class: verdict.class(),
        tables,
        execs: row.execs,
        total_secs: row.total_secs(),
        avg_millis: row.avg_millis(),
        p95_millis: row.quantile_95_picos.map(|p| p as f64 / 1e9),
        rows_sent: row.rows_sent,
        rows_examined: row.rows_examined,
        ratio: row.ratio(),
        no_index: row.no_index,
        scans: row.scans,
        sort_scans: row.sort_scans,
        tmp_disk: row.tmp_disk,
        errors: row.errors,
        lock_secs: row.lock_picos as f64 / 1e12,
        first_seen: row.first_seen.clone(),
        last_seen: row.last_seen.clone(),
    }
}

/// Functions that make the thing inside them unreachable by a b-tree index.
///
/// Not "every function MySQL has": only the ones that turn a column into a
/// computed value. An index stores the column, so a predicate on
/// `LOWER(col)` cannot use it -- MySQL would have to compute `LOWER` for
/// every row first, which is the scan this panel is trying to explain.
///
/// Deliberately a list and not a parser. This app does not parse SQL, and
/// §6.2 is the record of how badly verdict ordering goes when a rule reaches
/// past what it can actually see.
const WRAPPERS: &[&str] = &[
    "abs",
    "cast",
    "ceil",
    "ceiling",
    "char_length",
    "coalesce",
    "concat",
    "concat_ws",
    "convert",
    "date",
    "date_format",
    "day",
    "floor",
    "hex",
    "if",
    "ifnull",
    "json_contains",
    "json_extract",
    "json_overlaps",
    "json_search",
    "json_unquote",
    "left",
    "length",
    "lower",
    "lpad",
    "ltrim",
    "md5",
    "month",
    "nullif",
    "replace",
    "reverse",
    "right",
    "round",
    "rpad",
    "rtrim",
    "sha1",
    "sha2",
    "soundex",
    "substr",
    "substring",
    "trim",
    "unhex",
    "upper",
    "year",
];

/// Words that look like identifiers but name no column.
const NOT_A_COLUMN: &[&str] = &[
    "and", "as", "binary", "between", "case", "char", "datetime", "decimal", "distinct", "div",
    "else", "end", "escape", "false", "in", "integer", "interval", "is", "like", "mod", "not",
    "null", "or", "regexp", "rlike", "signed", "then", "time", "true", "unsigned", "when", "xor",
];

/// The next word in `hay` at or after `from`, and where it ended.
///
/// Backtick-quoted names count as one word with the quotes stripped, because
/// a digest renders both columns and function names that way:
/// `` `JSON_CONTAINS` ( `LOWER` ( COALESCE ( ALIAS , ? ) ) ) ``.
fn next_word(hay: &str, from: usize) -> Option<(&str, usize)> {
    let b = hay.as_bytes();
    let mut i = from;
    while i < b.len() {
        if b[i] == b'`' {
            let close = hay[i + 1..].find('`')? + i + 1;
            return Some((&hay[i + 1..close], close + 1));
        }
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            return Some((&hay[start..i], i));
        }
        i += 1;
    }
    None
}

/// The text inside the parentheses that open at `open`, or `None` if they
/// never close.
fn balanced(hay: &str, open: usize) -> Option<&str> {
    let b = hay.as_bytes();
    debug_assert_eq!(b[open], b'(');
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&hay[open + 1..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether `args` names a column, as opposed to being made of placeholders,
/// keywords and nested calls.
///
/// This is what tells `LOWER(COALESCE(ALIAS, ?))` -- which hides a column --
/// from `LOWER(?)`, which hides nothing and is perfectly indexable on the
/// other side of the comparison.
fn mentions_a_column(args: &str) -> bool {
    let mut at = 0;
    while let Some((word, end)) = next_word(args, at) {
        at = end;
        let w = word.to_ascii_lowercase();
        if WRAPPERS.contains(&w.as_str()) || NOT_A_COLUMN.contains(&w.as_str()) {
            continue;
        }
        return true;
    }
    false
}

/// Whether the `WHERE` clause wraps one of its own columns in a function.
///
/// # What it can get wrong
///
/// It reads the whole `WHERE` clause and does not know which side of a
/// comparison it is on, so `WHERE col = ? AND other = LOWER(col2)` answers
/// yes even though an index on `col` would still help. That is tolerable
/// because of **where this is called from**: only in the branch that was
/// about to say [`Verdict::MissingIndex`], which already requires the server
/// to be reading a hundred rows for every one it returns *and* to have found
/// no index on some executions. A sargable branch that was keeping the shape
/// healthy would not leave those counters looking like that.
///
/// It also only looks after `WHERE`, so a wrapped column in a `JOIN ... ON`
/// or a `HAVING` is missed. Those are false negatives: the verdict falls back
/// to `MissingIndex`, which is what it said before this existed.
fn wraps_a_column(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let Some(w) = lower.find(" where ") else {
        return false;
    };
    let hay = &lower[w + " where ".len()..];

    let mut at = 0;
    while let Some((word, end)) = next_word(hay, at) {
        at = end;
        if !WRAPPERS.contains(&word) {
            continue;
        }
        // The call's parenthesis, if this word is one.
        let rest = hay[end..].trim_start();
        if !rest.starts_with('(') {
            continue;
        }
        let open = end + hay[end..].find('(').expect("just checked");
        match balanced(hay, open) {
            Some(args) if mentions_a_column(args) => return true,
            _ => continue,
        }
    }
    false
}

fn verdict_of(
    row: &DigestRow,
    cands: &[String],
    tables: &[String],
    known: &KnownTables,
) -> Verdict {
    // 1. Not ours at all.
    if is_platform(cands) {
        return Verdict::PlatformInternal;
    }
    if is_chatter(&row.text) {
        return Verdict::DriverChatter;
    }

    if row.execs == 0 {
        return Verdict::Unclassified;
    }

    // 2. It is not reading, so nothing about reading explains it.
    if row.rows_examined == 0 && row.avg_micros() > WAITING_AVG_US {
        return Verdict::WaitingNotReading;
    }

    // 3. A view's predicate cannot reach the base tables, so an index on the
    //    name in the query does not exist to be proposed.
    if tables
        .iter()
        .any(|t| known.get(t).copied().unwrap_or(false))
    {
        return Verdict::ViewBlocksFilter;
    }

    // 4. It returns what it reads. A scan is the right plan.
    if let Some(ratio) = row.ratio() {
        if ratio <= RATIO_READS_WHAT_IT_RETURNS && row.scans >= row.execs {
            return Verdict::NoIndexWillHelp;
        }
    }

    // 5. Only now. And an index can only be used if the filter leaves the
    //    column alone, so the last thing checked before proposing one is
    //    whether a proposal could be used at all.
    if let Some(ratio) = row.ratio() {
        if ratio > RATIO_WANTS_INDEX && (row.no_index > 0 || row.scans > 0) {
            if wraps_a_column(&row.text) {
                return Verdict::PredicateNotIndexable;
            }
            return Verdict::MissingIndex;
        }
    }

    Verdict::Unclassified
}

/// Several shapes on one table that ran the same number of times and started
/// together.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopFinding {
    pub table: String,
    /// Digests of the members, most expensive first.
    pub digests: Vec<String>,
    pub execs_low: u64,
    pub execs_high: u64,
    /// The spread as a fraction of the larger count.
    pub spread: f64,
    /// Microseconds between the earliest and latest first sighting.
    pub first_seen_spread_us: u64,
    pub total_secs: f64,
}

/// Find read-then-write loops.
///
/// Grouped by referenced table, because a loop is a property of several rows
/// and no single row of the digest table can show it. A cluster needs at least
/// one write in it: three `SELECT`s on one table at the same frequency are a
/// dashboard refreshing, not a loop worth reporting.
///
/// Only the *findings* come back. A member keeps whatever verdict
/// [`classify`] gave it, except one that would be [`Verdict::Unclassified`] —
/// see [`promote_loop_members`]. The expensive read in a loop is usually also
/// a genuine `MissingIndex` and saying so is more useful than relabelling it.
pub fn loops(rows: &[DigestRow], findings: &[Finding]) -> Vec<LoopFinding> {
    let mut by_table: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, f) in findings.iter().enumerate() {
        if f.class != Class::Application || f.execs == 0 {
            continue;
        }
        for t in &f.tables {
            by_table.entry(t.as_str()).or_default().push(i);
        }
    }

    let mut out: Vec<LoopFinding> = Vec::new();

    for (table, idxs) in by_table {
        if idxs.len() < 2 {
            continue;
        }
        // Greedy clustering on execution count. The counts are already within
        // a tenth of a percent of each other or they are not a cluster, so
        // anything cleverer than "walk them in order" would be decoration.
        let mut sorted = idxs.clone();
        sorted.sort_by_key(|&i| std::cmp::Reverse(findings[i].execs));

        let mut cluster: Vec<usize> = Vec::new();
        for &i in &sorted {
            match cluster.last() {
                None => cluster.push(i),
                Some(&prev) => {
                    if close_enough(findings[prev].execs, findings[i].execs) {
                        cluster.push(i);
                    } else {
                        if let Some(found) = finish(table, &cluster, rows, findings) {
                            out.push(found);
                        }
                        cluster = vec![i];
                    }
                }
            }
        }
        if let Some(found) = finish(table, &cluster, rows, findings) {
            out.push(found);
        }
    }

    out.sort_by(|a, b| b.total_secs.total_cmp(&a.total_secs));
    out
}

fn close_enough(a: u64, b: u64) -> bool {
    let high = a.max(b);
    if high == 0 {
        return false;
    }
    let spread = high - a.min(b);
    (spread as f64 / high as f64) < LOOP_TOLERANCE
}

fn finish(
    table: &str,
    cluster: &[usize],
    rows: &[DigestRow],
    findings: &[Finding],
) -> Option<LoopFinding> {
    if cluster.len() < 2 {
        return None;
    }

    let digests: HashSet<&str> = cluster
        .iter()
        .map(|&i| findings[i].digest.as_str())
        .collect();
    let members: Vec<&DigestRow> = rows
        .iter()
        .filter(|r| digests.contains(r.digest.as_str()))
        .collect();
    if members.len() < 2 {
        return None;
    }

    // A loop writes. Reads alone at one frequency are something on a timer.
    let writes = members
        .iter()
        .any(|r| exec::classify(&r.text) == exec::StatementKind::Modify);
    if !writes {
        return None;
    }

    let first_lo = members.iter().map(|r| r.first_seen_us).min()?;
    let first_hi = members.iter().map(|r| r.first_seen_us).max()?;
    let seen_spread = first_hi.saturating_sub(first_lo);
    if seen_spread > LOOP_FIRST_SEEN_US {
        return None;
    }

    let lo = cluster.iter().map(|&i| findings[i].execs).min()?;
    let hi = cluster.iter().map(|&i| findings[i].execs).max()?;

    let mut ordered: Vec<usize> = cluster.to_vec();
    ordered.sort_by(|&a, &b| findings[b].total_secs.total_cmp(&findings[a].total_secs));

    Some(LoopFinding {
        table: table.to_string(),
        digests: ordered
            .iter()
            .map(|&i| findings[i].digest.clone())
            .collect(),
        execs_low: lo,
        execs_high: hi,
        spread: if hi == 0 {
            0.0
        } else {
            (hi - lo) as f64 / hi as f64
        },
        first_seen_spread_us: seen_spread,
        total_secs: ordered.iter().map(|&i| findings[i].total_secs).sum(),
    })
}

/// Give the loop's verdict to members that had nothing better to say.
///
/// A member already carrying [`Verdict::MissingIndex`] keeps it: the read in a
/// loop usually *is* missing an index, and "fix the loop" and "fix the index"
/// are both true and separately actionable. Only `Unclassified` is overwritten,
/// which on the server this was measured against is exactly the two `UPDATE`s.
pub fn promote_loop_members(findings: &mut [Finding], found: &[LoopFinding]) {
    let in_a_loop: HashSet<&str> = found
        .iter()
        .flat_map(|l| l.digests.iter().map(|d| d.as_str()))
        .collect();
    for f in findings.iter_mut() {
        if f.verdict == Verdict::Unclassified && in_a_loop.contains(f.digest.as_str()) {
            f.verdict = Verdict::RowByRowLoop;
            f.class = Verdict::RowByRowLoop.class();
        }
    }
}

/// How many times each table was written, over **every** digest in the ring.
///
/// This is what prices an index: a table taking eighty million writes pays for
/// every index on it eighty million times, and that is the difference between
/// a two-column index being obvious and a six-column covering index being a
/// mistake. Both halves come out of one query.
///
/// Totalled over all rows rather than the shipped top-N, because a top-N here
/// would silently turn a total into a sample — the fault §3.1 recorded in the
/// probe that started this stage.
pub fn write_pressure(rows: &[DigestRow], known: &KnownTables) -> HashMap<String, u64> {
    let mut out: HashMap<String, u64> = HashMap::new();
    for row in rows {
        if exec::classify(&row.text) != exec::StatementKind::Modify {
            continue;
        }
        let cands = candidates(&row.text);
        if is_platform(&cands) {
            continue;
        }
        for t in cands.into_iter().filter(|c| known.contains_key(c)) {
            *out.entry(t).or_insert(0) += row.execs;
        }
    }
    out
}

/// Everything the panel is given for one connection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub shapes: Vec<Finding>,
    pub loops: Vec<LoopFinding>,
    /// Table name to writes over the window.
    pub writes: HashMap<String, u64>,
    /// Digests in the ring, before anything was dropped for shipping.
    pub total_shapes: usize,
    pub application_shapes: usize,
    pub chatter_shapes: usize,
    pub platform_shapes: usize,
    /// Seconds of server time in each class, over every row read.
    pub application_secs: f64,
    pub chatter_secs: f64,
    pub platform_secs: f64,
    /// `true` when the server is 8.0 or later and p95 is therefore populated.
    pub has_quantiles: bool,
}

/// Turn a whole ring into the report the panel renders.
pub fn report(rows: &[DigestRow], known: &KnownTables) -> Report {
    let mut findings: Vec<Finding> = rows.iter().map(|r| classify(r, known)).collect();
    let found = loops(rows, &findings);
    promote_loop_members(&mut findings, &found);

    let writes = write_pressure(rows, known);

    let count = |c: Class| findings.iter().filter(|f| f.class == c).count();
    let secs = |c: Class| -> f64 {
        findings
            .iter()
            .filter(|f| f.class == c)
            .map(|f| f.total_secs)
            .sum()
    };

    let report = Report {
        total_shapes: findings.len(),
        application_shapes: count(Class::Application),
        chatter_shapes: count(Class::Chatter),
        platform_shapes: count(Class::Platform),
        application_secs: secs(Class::Application),
        chatter_secs: secs(Class::Chatter),
        platform_secs: secs(Class::Platform),
        has_quantiles: rows.iter().any(|r| r.quantile_95_picos.is_some()),
        loops: found,
        writes,
        shapes: Vec::new(),
    };

    findings.sort_by(|a, b| b.total_secs.total_cmp(&a.total_secs));
    findings.truncate(SHAPES_SENT);

    Report {
        shapes: findings,
        ..report
    }
}

/// The major version, for the columns that only exist from MySQL 8.0.
///
/// Aurora reports things like `8.0.mysql_aurora.3.04.0`, so only the leading
/// integer is read and anything unparseable is treated as old. Guessing high
/// would cost an error on the one query this feature is made of; guessing low
/// costs a column.
fn major_version(version: Option<&str>) -> u32 {
    version
        .and_then(|v| v.split('.').next())
        .and_then(|n| n.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// Read the whole digest ring.
///
/// **No `LIMIT`.** The table is in memory and bounded by
/// `performance_schema_digests_size` — 10,000 rows by default — and
/// [`write_pressure`] is a total that a top-N would quietly turn into a
/// sample. Bounding happens in [`report`], after the aggregation, where it
/// only costs display.
///
/// `DIGEST_TEXT IS NOT NULL` drops exactly one row: the bucket the server uses
/// when the ring overflows, which is a counter rather than a statement.
/// How long a read of this panel's may take before the **server** stops it.
///
/// Every query in this module runs on `ServerConn::mysql_meta`, which is one
/// connection behind one mutex, shared with the schema tree and autocomplete.
/// A statement that never returns holds that mutex for the rest of the
/// session, and there is no cancel path for it at all: `KILL QUERY` targets a
/// *tab's* connection id, and this connection has none. Schema browsing and
/// completion would simply stop working, with nothing to click.
///
/// So the deadline is handed to the server rather than kept here. A client-side
/// timeout would be worse than useless: it abandons a connection mid-protocol,
/// and this is the connection that cannot be abandoned.
///
/// 15 seconds is well past interactive and well short of a lunch break. The
/// panel's own reads answer in milliseconds; this is for the ones that never
/// answer at all.
///
/// **Measured, not hypothetical.** `information_schema.STATISTICS` over two
/// schemas never returned on the UAT instance (2026-11-05). See section 6.9 of
/// `docs/stage-31-where-the-time-goes.md`.
const DEADLINE_MS: u32 = 15_000;

/// `MAX_EXECUTION_TIME` as an optimizer hint.
///
/// Built from [`DEADLINE_MS`] rather than written out, so the number in the SQL
/// and the number in the sentence shown when it fires cannot drift apart.
fn deadline() -> String {
    format!("/*+ MAX_EXECUTION_TIME({DEADLINE_MS}) */")
}

/// `ER_QUERY_TIMEOUT`. It can only reach us from the hint above -- nothing else
/// in this app sets a statement deadline.
const DEADLINE_CODE: &str = "3024";

/// The digest read.
///
/// Pure so that the version gate and the deadline are both testable without a
/// server — which is the half of this module that had no test at all until the
/// probe was finally run against one.
///
/// `QUANTILE_95` arrived in MySQL 8.0 — the same version-gated column list
/// `schema.rs` already builds for `VIEW_TABLE_USAGE`. Tail latency matters more
/// than the mean for a shape that is usually fine and occasionally terrible, so
/// it is taken where it exists rather than asked for and recovered from.
///
/// A server too old to understand the hint ignores it with a warning instead of
/// failing, so unlike the quantile it needs no gate of its own.
fn digest_sql(eight: bool) -> String {
    let quantile = if eight { "QUANTILE_95" } else { "NULL" };
    format!(
        "SELECT {} \
                SCHEMA_NAME AS `db`, \
                DIGEST AS `digest`, \
                DIGEST_TEXT AS `text`, \
                COUNT_STAR AS `execs`, \
                SUM_TIMER_WAIT AS `total_picos`, \
                AVG_TIMER_WAIT AS `avg_picos`, \
                SUM_LOCK_TIME AS `lock_picos`, \
                SUM_ERRORS AS `errors`, \
                SUM_ROWS_SENT AS `rows_sent`, \
                SUM_ROWS_EXAMINED AS `rows_examined`, \
                SUM_NO_INDEX_USED AS `no_index`, \
                SUM_SELECT_SCAN AS `scans`, \
                SUM_SORT_SCAN AS `sort_scans`, \
                SUM_CREATED_TMP_DISK_TABLES AS `tmp_disk`, \
                {quantile} AS `q95`, \
                DATE_FORMAT(FIRST_SEEN, '%Y-%m-%d %H:%i:%s') AS `first_seen`, \
                DATE_FORMAT(LAST_SEEN, '%Y-%m-%d %H:%i:%s') AS `last_seen`, \
                CAST(UNIX_TIMESTAMP(FIRST_SEEN) * 1000000 AS UNSIGNED) AS `first_us`, \
                CAST(UNIX_TIMESTAMP(LAST_SEEN) * 1000000 AS UNSIGNED) AS `last_us` \
           FROM performance_schema.events_statements_summary_by_digest \
          WHERE DIGEST_TEXT IS NOT NULL \
          ORDER BY SUM_TIMER_WAIT DESC",
        deadline()
    )
}

pub(crate) async fn mysql_workload(server: &ServerConn) -> Result<Vec<DigestRow>, String> {
    let sql = digest_sql(major_version(server.server_version.as_deref()) >= 8);

    let rows = {
        let mut meta = server.mysql_meta().await?;
        // Safe to assert: the only interpolation is `quantile`, which is one
        // of two literals chosen by this function from the server's own
        // version string. Nothing a user typed reaches this query, and it
        // takes no binds because it has no predicate.
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(&mut *meta)
            .await
            .map_err(|e| friendly(&e))?
    };

    let num = |r: &sqlx::mysql::MySqlRow, col: &str| -> u64 {
        r.try_get::<Option<u64>, _>(col).ok().flatten().unwrap_or(0)
    };

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let digest: String = r.try_get::<Option<String>, _>("digest").ok().flatten()?;
            let text: String = r.try_get::<Option<String>, _>("text").ok().flatten()?;
            Some(DigestRow {
                schema: r.try_get::<Option<String>, _>("db").ok().flatten(),
                digest,
                text,
                execs: num(&r, "execs"),
                total_picos: num(&r, "total_picos"),
                avg_picos: num(&r, "avg_picos"),
                lock_picos: num(&r, "lock_picos"),
                errors: num(&r, "errors"),
                rows_sent: num(&r, "rows_sent"),
                rows_examined: num(&r, "rows_examined"),
                no_index: num(&r, "no_index"),
                scans: num(&r, "scans"),
                sort_scans: num(&r, "sort_scans"),
                tmp_disk: num(&r, "tmp_disk"),
                quantile_95_picos: r.try_get::<Option<u64>, _>("q95").ok().flatten(),
                first_seen: r
                    .try_get::<Option<String>, _>("first_seen")
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                last_seen: r
                    .try_get::<Option<String>, _>("last_seen")
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                first_seen_us: num(&r, "first_us"),
                last_seen_us: num(&r, "last_us"),
            })
        })
        .collect())
}

/// One index, as `information_schema.STATISTICS` describes it.
#[derive(Debug, Clone, Default)]
pub struct IndexRow {
    pub database: String,
    pub table: String,
    pub name: String,
    /// Key parts in key order. `None` is a **functional key part** -- an
    /// expression rather than a column.
    ///
    /// `information_schema.STATISTICS` reports `COLUMN_NAME` as `NULL` for
    /// one of those and does not say what the expression is. Measured on
    /// 2026-11-06: six such parts across both servers, including
    /// `uq_reconciliation_config_scope`, whose first two parts are
    /// expressions and whose third is `entity_type`.
    ///
    /// They were silently dropped before, which cost five true redundancy
    /// findings and described that index as leading with `entity_type` when
    /// it does not. `None` is not "no column": it is "a key part we cannot
    /// read", and the two rules below refuse to conclude anything that would
    /// need to know what it is.
    pub columns: Vec<Option<String>>,
    pub unique: bool,
}

/// How often an index has been read since the server started.
#[derive(Debug, Clone, Default)]
pub struct IndexUse {
    pub database: String,
    pub table: String,
    pub name: String,
    pub reads: u64,
}

/// How the indexes on a connection divide (Stage 33).
///
/// The inventory has always reported how many findings it has and never what
/// that is a fraction of. On UAT it is 245 of 1,025 -- a quarter of every
/// index on the instance -- and a panel proposing to drop a quarter of an
/// instance's indexes should say so before the first `DROP INDEX` is read.
///
/// Counted from the **usage rows**, which is what the never-read rule is
/// applied to, so `seen` is the denominator the split is actually true of.
/// The catalogue agreed with it exactly on UAT (1,025 both ways, §8.2.1), and
/// that is a measurement rather than a guarantee -- so the two are not mixed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexUsageSplit {
    pub seen: u64,
    pub read: u64,
    pub unread: u64,
    /// Skipped by the rule rather than by the data, so a part of its own.
    pub primary_keys: u64,
}

/// Divide the usage rows the same way [`index_findings`] does.
///
/// The predicate is spelled out once there and once here, which is a copy --
/// and the test below is what stops the two drifting, because a split whose
/// `unread` disagreed with the number of never-read findings beside it would
/// be worse than no split at all.
pub fn index_usage_split(uses: &[IndexUse]) -> IndexUsageSplit {
    let mut split = IndexUsageSplit {
        seen: uses.len() as u64,
        ..Default::default()
    };
    for row in uses {
        if row.name.eq_ignore_ascii_case("PRIMARY") {
            split.primary_keys += 1;
        } else if row.reads > 0 {
            split.read += 1;
        } else {
            split.unread += 1;
        }
    }
    split
}

/// Why an index is being questioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexFault {
    /// Not read once since the counters started. Pure write cost.
    NeverRead,
    /// A leading prefix of another index on the same table, so it earns
    /// nothing the longer one does not already give.
    Redundant,
    /// A foreign key whose child column has no index to support it.
    UnindexedForeignKey,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexFinding {
    pub fault: IndexFault,
    pub database: String,
    pub table: String,
    pub name: String,
    pub columns: Vec<String>,
    /// For `Redundant`: the index that already covers this one.
    pub covered_by: Option<String>,
    /// Writes against this table over the window, from the same digests —
    /// what an index actually costs. `None` when the workload did not mention
    /// the table at all.
    pub writes: Option<u64>,
    /// The statement to run, which the person runs themselves.
    pub statement: String,
}

/// Indexes that are a leading prefix of another on the same table.
///
/// Pure structure: no workload needed, so this half is right whatever the
/// server has been doing lately, and it works on a connection that cannot read
/// `performance_schema` at all.
///
/// **A unique index is never called redundant by a non-unique one.** It is not
/// there to be read; it is there to refuse duplicate rows, and dropping it
/// changes what the table accepts. That asymmetry is the whole reason this is
/// not a one-line prefix comparison.
pub fn redundant(indexes: &[IndexRow]) -> Vec<(IndexRow, String)> {
    let mut out: Vec<(IndexRow, String)> = Vec::new();

    for a in indexes {
        if a.name.eq_ignore_ascii_case("PRIMARY") {
            continue;
        }
        for b in indexes {
            if std::ptr::eq(a, b)
                || a.database != b.database
                || !a.table.eq_ignore_ascii_case(&b.table)
                || a.name == b.name
            {
                continue;
            }
            // `a` must be a strict leading prefix of `b`.
            if a.columns.len() >= b.columns.len() {
                continue;
            }
            // **An expression is opaque, and two opaque parts are not known
            // to be equal.** So `a` may not contain one: proposing that an
            // index be dropped means claiming another one covers it, and that
            // claim cannot be made about a key part whose expression
            // `STATISTICS` does not report. An expression in `b` *beyond*
            // `a`'s length is harmless -- nothing is being compared to it.
            let prefix = a.columns.len() <= b.columns.len()
                && a.columns
                    .iter()
                    .zip(b.columns.iter())
                    .all(|(x, y)| match (x, y) {
                        (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
                        _ => false,
                    });
            if !prefix {
                continue;
            }
            // Uniqueness is a constraint, not an access path.
            if a.unique && !b.unique {
                continue;
            }
            out.push((a.clone(), b.name.clone()));
            break;
        }
    }
    out
}

/// Foreign keys whose child columns have no index to support them.
///
/// Every parent delete, and every update of a referenced key, has to find the
/// matching child rows. Without an index that is a scan of the child table —
/// and it is held under a lock while it runs, so the cost is not only the read.
///
/// **InnoDB creates an index for a foreign key it defines itself**, so this is
/// not usually a long list. What it catches is the case InnoDB does not cover:
/// a constraint added by a later `ALTER` against a column that already had
/// one, where the index it would have created was judged already present and
/// then later dropped or renamed.
///
/// An index counts as support when the key's columns are its **leading
/// prefix**, in order. A key on `(a, b)` is supported by an index on
/// `(a, b, c)` and not by one on `(b, a)`.
///
/// A functional key part cannot support a column. `STATISTICS` does not say
/// what the expression is, so an index on `((expr), (expr), entity_type)`
/// supports nothing this can name -- and before expressions were represented
/// at all, that index read as one on `(entity_type)` and would have been
/// counted as support for a foreign key it does not serve.
pub fn unindexed_foreign_keys(keys: &[IndexRow], indexes: &[IndexRow]) -> Vec<IndexRow> {
    keys.iter()
        .filter(|key| {
            !indexes.iter().any(|idx| {
                idx.database == key.database
                    && idx.table.eq_ignore_ascii_case(&key.table)
                    && idx.columns.len() >= key.columns.len()
                    && key
                        .columns
                        .iter()
                        .zip(idx.columns.iter())
                        .all(|(k, i)| match (k, i) {
                            (Some(k), Some(i)) => k.eq_ignore_ascii_case(i),
                            _ => false,
                        })
            })
        })
        .cloned()
        .collect()
}

/// Turn the two structural reads plus the workload into findings.
///
/// `writes` comes from [`write_pressure`], so every finding is priced by the
/// same data that found it — a never-read index on a table taking eighty
/// million writes is a very different proposition from one on a table nobody
/// writes to, and the list says which.
pub fn index_findings(
    indexes: &[IndexRow],
    uses: &[IndexUse],
    unindexed_keys: &[IndexRow],
    writes: &HashMap<String, u64>,
) -> Vec<IndexFinding> {
    let cost = |table: &str| writes.get(&table.to_ascii_lowercase()).copied();
    let mut out: Vec<IndexFinding> = Vec::new();

    for use_row in uses {
        if use_row.reads > 0 || use_row.name.eq_ignore_ascii_case("PRIMARY") {
            continue;
        }
        let columns = indexes
            .iter()
            .find(|i| {
                i.table.eq_ignore_ascii_case(&use_row.table)
                    && i.name.eq_ignore_ascii_case(&use_row.name)
            })
            .map(|i| shown(&i.columns))
            .unwrap_or_default();
        out.push(IndexFinding {
            fault: IndexFault::NeverRead,
            statement: drop_index(&use_row.database, &use_row.table, &use_row.name),
            database: use_row.database.clone(),
            table: use_row.table.clone(),
            name: use_row.name.clone(),
            columns,
            covered_by: None,
            writes: cost(&use_row.table),
        });
    }

    for (index, covered_by) in redundant(indexes) {
        out.push(IndexFinding {
            fault: IndexFault::Redundant,
            statement: drop_index(&index.database, &index.table, &index.name),
            writes: cost(&index.table),
            database: index.database,
            table: index.table,
            name: index.name,
            columns: shown(&index.columns),
            covered_by: Some(covered_by),
        });
    }

    for key in unindexed_keys {
        // `KEY_COLUMN_USAGE` has no functional parts -- a foreign key is on
        // columns -- but this writes a `CREATE INDEX` somebody will run, so
        // anything unreadable here is skipped rather than guessed at.
        let Some(names) = key
            .columns
            .iter()
            .map(|c| c.clone())
            .collect::<Option<Vec<String>>>()
        else {
            continue;
        };
        let cols = names
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(IndexFinding {
            fault: IndexFault::UnindexedForeignKey,
            statement: format!(
                "CREATE INDEX {} ON {}.{} ({cols});",
                quote(&format!("idx_{}_{}", key.table, names.join("_"))),
                quote(&key.database),
                quote(&key.table),
            ),
            writes: cost(&key.table),
            database: key.database.clone(),
            table: key.table.clone(),
            name: key.name.clone(),
            columns: names,
            covered_by: None,
        });
    }

    out
}

/// Key parts as a person should read them.
///
/// An expression is named rather than left blank. `IndexFinding::columns`
/// crosses into the UI, where a blank would read as "this index has no
/// columns" -- a different and wronger claim than "this part is an
/// expression we cannot see".
fn shown(columns: &[Option<String>]) -> Vec<String> {
    columns
        .iter()
        .map(|c| c.clone().unwrap_or_else(|| "(expression)".into()))
        .collect()
}

/// A MySQL identifier, with an embedded backtick doubled.
fn quote(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

fn drop_index(database: &str, table: &str, name: &str) -> String {
    format!(
        "DROP INDEX {} ON {}.{};",
        quote(name),
        quote(database),
        quote(table)
    )
}

/// What six cheap queries said about whether this server can be read at all.
///
/// Every field is an `Option` because **every one of them can fail
/// separately**, and which one failed is the diagnosis. A `None` on
/// [`digest_consumer`](Readings::digest_consumer) while
/// [`enabled`](Readings::enabled) is `Some(true)` means the instrumentation is
/// running and this connection is not allowed to read it — a different problem
/// with a different fix from the same empty grid.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readings {
    /// `@@performance_schema`. Readable by anyone.
    pub enabled: Option<bool>,
    /// The `statements_digest` row of `setup_consumers`. `None` means the
    /// read was refused.
    pub digest_consumer: Option<bool>,
    /// Enabled `statement/%` rows of `setup_instruments`.
    pub statement_instruments: Option<u64>,
    pub uptime_secs: Option<u64>,
    pub digests_lost: Option<u64>,
    pub digests_present: Option<u64>,
}

/// The ladder's answer: whether to show a workload, and what to say if not.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    /// `true` only when there is something to read **and** it can be trusted.
    pub usable: bool,
    /// One sentence. Never "no data".
    pub headline: String,
    /// What to change, and by whom.
    pub detail: String,
    pub readings: Readings,
}

/// Work down the ladder and name the first rung that answered wrong.
///
/// Pure, so the sentence a person reads for a server nobody has is testable
/// without one. §4.10 and Phase 5 of the Stage 31 doc.
///
/// The order is the point: four different causes produce the same empty grid,
/// and a panel that says "no data" for all four is the thing this is here to
/// avoid.
pub fn diagnose(r: &Readings) -> Diagnosis {
    let say = |usable: bool, headline: &str, detail: &str| Diagnosis {
        usable,
        headline: headline.to_string(),
        detail: detail.to_string(),
        readings: r.clone(),
    };

    // 1. Switched off outright. Nothing else is worth reporting.
    if r.enabled == Some(false) {
        return say(
            false,
            "performance_schema is switched off on this server.",
            "On RDS and Aurora this is a parameter-group value and the instance has to \
             restart to pick it up — neither of which a database client should be doing. \
             Enabling Performance Insights turns it on as a side effect.",
        );
    }

    // 2. On, but this connection cannot look. Told apart from "off" because
    //    the fix is a grant rather than a restart.
    if r.digest_consumer.is_none() && r.digests_present.is_none() {
        return say(
            false,
            "This connection cannot read performance_schema.",
            "One grant fixes it, and it gives away nothing: a digest holds no literal \
             values and no row data. GRANT SELECT ON performance_schema.* TO <user>.",
        );
    }

    // 3. Collecting nothing, for two different reasons.
    if r.digest_consumer == Some(false) {
        return say(
            false,
            "The statements_digest consumer is off, so no shape is ever recorded.",
            "performance_schema can be ON with this one consumer disabled, which is the \
             most common cause of a readable but permanently empty digest table. It is a \
             server setting, not something this app should change.",
        );
    }
    if r.statement_instruments == Some(0) {
        return say(
            false,
            "Statement instrumentation is disabled, so the consumer has nothing to consume.",
            "The consumer is on but every statement/% instrument is off. Same empty table, \
             a different switch.",
        );
    }

    // 4. Working, with nothing in it yet — which is a finding about the
    //    server's age, not a failure of this panel.
    if r.digests_present == Some(0) {
        let fresh = r.uptime_secs.is_some_and(|u| u < 3_600);
        return if fresh {
            say(
                false,
                "The counters were reset less than an hour ago.",
                "A restart or a TRUNCATE empties the digest table. Instrumentation is \
                 working — there is simply nothing in it yet. Come back once the server \
                 has done some work.",
            )
        } else {
            say(
                false,
                "performance_schema is collecting, and has recorded nothing.",
                "Everything is switched on and the table is empty, which on a server that \
                 has been up this long means no statement has run through it. An idle \
                 server is the likeliest answer.",
            )
        };
    }

    // 5. Readable, and worth reading — but say so if it is a sample.
    if r.digests_lost.is_some_and(|l| l > 0) {
        return say(
            true,
            "The digest ring has overflowed, so this list is a sample.",
            "Performance_schema_digest_lost is above zero: shapes were dropped rather than \
             counted, and more than ten thousand distinct shapes usually means literals are \
             not being parameterised somewhere. Treat totals as lower bounds.",
        );
    }

    say(
        true,
        "Reading the server's own counters.",
        "performance_schema is on, readable, and nothing has been lost from the ring.",
    )
}

/// Run the ladder's six queries, tolerating each one's failure separately.
pub(crate) async fn mysql_diagnose(server: &ServerConn) -> Readings {
    let mut out = Readings::default();
    let mut meta = match server.mysql_meta().await {
        Ok(m) => m,
        Err(_) => return out,
    };

    if let Ok(row) = sqlx::query("SELECT @@performance_schema AS v")
        .fetch_one(&mut *meta)
        .await
    {
        out.enabled = row.try_get::<i64, _>("v").ok().map(|v| v != 0);
    }

    if let Ok(row) = sqlx::query(
        "SELECT enabled AS v FROM performance_schema.setup_consumers \
          WHERE name = 'statements_digest'",
    )
    .fetch_one(&mut *meta)
    .await
    {
        out.digest_consumer = row
            .try_get::<String, _>("v")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("YES"));
    }

    if let Ok(row) = sqlx::query(
        "SELECT COUNT(*) AS v FROM performance_schema.setup_instruments \
          WHERE name LIKE 'statement/%' AND enabled = 'YES'",
    )
    .fetch_one(&mut *meta)
    .await
    {
        out.statement_instruments = row.try_get::<i64, _>("v").ok().map(|v| v.max(0) as u64);
    }

    // `SHOW GLOBAL STATUS` rather than `performance_schema.global_status`, so
    // that uptime is still answerable on the connection that cannot read
    // performance_schema — which is exactly the connection that needs the
    // diagnosis most.
    out.uptime_secs = show_status(&mut meta, "Uptime").await;
    out.digests_lost = show_status(&mut meta, "Performance_schema_digest_lost").await;

    if let Ok(row) = sqlx::query(
        "SELECT COUNT(*) AS v \
           FROM performance_schema.events_statements_summary_by_digest \
          WHERE DIGEST_TEXT IS NOT NULL",
    )
    .fetch_one(&mut *meta)
    .await
    {
        out.digests_present = row.try_get::<i64, _>("v").ok().map(|v| v.max(0) as u64);
    }

    out
}

/// `IN (?, ?, ?)` with one placeholder per database.
///
/// Built rather than interpolated even though these names come from the
/// server's own counters and the schema cache: a bind is the habit, and a
/// habit that has an exception is not a habit.
fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(", ")
}

/// Why the index inventory has nothing to show.
///
/// Worth its own sentence rather than relaying `ER_QUERY_TIMEOUT`, because the
/// distinction the person needs is *which half failed*: the workload above is
/// still true, and saying so stops a slow `information_schema` from reading as
/// a broken panel.
fn inventory_refused(e: &sqlx::Error) -> String {
    let code = match e {
        sqlx::Error::Database(db) => db.code().map(|c| c.to_string()),
        _ => None,
    };
    refusal_text(code.as_deref(), || friendly(e))
}

/// Split from [`inventory_refused`] so the deadline branch is reachable in a
/// test: a `sqlx::Error::Database` cannot be constructed outside the driver, so
/// a test that went through the `sqlx::Error` would only ever exercise the
/// fallback and claim to have checked both.
fn refusal_text(code: Option<&str>, otherwise: impl FnOnce() -> String) -> String {
    if code == Some(DEADLINE_CODE) {
        return format!(
            "Listing indexes took longer than the {} seconds this read is given, so the \
             server stopped it. On some servers information_schema.STATISTICS has to open \
             every table in the schema to answer. The workload above is unaffected: that \
             comes from a table already in memory.",
            DEADLINE_MS / 1000
        );
    }
    otherwise()
}

/// Every index on the named databases, columns in key order.
///
/// **The one that made [`DEADLINE`] necessary.** On the UAT instance this never
/// came back: not slow, not refused, just gone, taking the shared connection
/// with it (2026-11-05).
fn indexes_sql(dbs: usize) -> String {
    format!(
        "SELECT {} \
                TABLE_SCHEMA AS `db`, TABLE_NAME AS `tbl`, INDEX_NAME AS `idx`, \
                COLUMN_NAME AS `col`, NON_UNIQUE AS `non_unique` \
           FROM information_schema.STATISTICS \
          WHERE TABLE_SCHEMA IN ({}) \
          ORDER BY TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        deadline(),
        placeholders(dbs)
    )
}

/// How often each index has actually been read.
fn index_use_sql(dbs: usize) -> String {
    format!(
        "SELECT {} \
                OBJECT_SCHEMA AS `db`, OBJECT_NAME AS `tbl`, INDEX_NAME AS `idx`, \
                COUNT_READ AS `reads` \
           FROM performance_schema.table_io_waits_summary_by_index_usage \
          WHERE INDEX_NAME IS NOT NULL AND OBJECT_SCHEMA IN ({})",
        deadline(),
        placeholders(dbs)
    )
}

/// Every foreign key's child columns, in order.
fn foreign_keys_sql(dbs: usize) -> String {
    format!(
        "SELECT {} \
                CONSTRAINT_SCHEMA AS `db`, TABLE_NAME AS `tbl`, \
                CONSTRAINT_NAME AS `name`, COLUMN_NAME AS `col` \
           FROM information_schema.KEY_COLUMN_USAGE \
          WHERE REFERENCED_TABLE_NAME IS NOT NULL \
            AND CONSTRAINT_SCHEMA IN ({}) \
          ORDER BY CONSTRAINT_SCHEMA, TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
        deadline(),
        placeholders(dbs)
    )
}

/// Every index on the named databases, columns in key order.
///
/// `information_schema` only — so this works on a connection that cannot read
/// `performance_schema`, which is the whole point of the structural half.
pub(crate) async fn mysql_indexes(
    server: &ServerConn,
    dbs: &[String],
) -> Result<Vec<IndexRow>, String> {
    if dbs.is_empty() {
        return Ok(Vec::new());
    }
    let sql = indexes_sql(dbs.len());

    let rows = {
        let mut meta = server.mysql_meta().await?;
        // Safe to assert: the only interpolation is the placeholder list,
        // whose length is a count. Every value is bound below.
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        for db in dbs {
            q = q.bind(db);
        }
        q.fetch_all(&mut *meta)
            .await
            .map_err(|e| inventory_refused(&e))?
    };

    let mut out: Vec<IndexRow> = Vec::new();
    for r in rows {
        let (Ok(db), Ok(tbl), Ok(idx)) = (
            r.try_get::<String, _>("db"),
            r.try_get::<String, _>("tbl"),
            r.try_get::<String, _>("idx"),
        ) else {
            continue;
        };
        // **`Option<String>`, not `String`.** `COLUMN_NAME` is `NULL` for a
        // functional key part, and reading it as a `String` made the whole
        // row fail to decode -- so the part was dropped and the index arrived
        // narrower than it is. Measured against both servers on 2026-11-06:
        // six parts lost, five true redundancy findings with them, and
        // `uq_reconciliation_config_scope` reported as leading with its third
        // key part.
        let col = r.try_get::<Option<String>, _>("col").unwrap_or(None);
        let unique = r.try_get::<i64, _>("non_unique").unwrap_or(1) == 0;
        fold_part(&mut out, db, tbl, idx, col, unique);
    }
    Ok(out)
}

/// Add one key part to the index being built, or start a new index.
///
/// Split out of [`mysql_indexes`] so the `None` case is reachable in a test.
/// A `sqlx::Row` cannot be constructed outside the driver, so a test that
/// went through the decode would only ever exercise whichever branch the
/// fixture happened to take -- which is exactly how a dropped functional key
/// part survived to be found by a live server instead.
///
/// The query is ordered by index and key position, so a continuing index is
/// always the last one added.
fn fold_part(
    out: &mut Vec<IndexRow>,
    db: String,
    tbl: String,
    idx: String,
    col: Option<String>,
    unique: bool,
) {
    match out.last_mut() {
        Some(last) if last.database == db && last.table == tbl && last.name == idx => {
            last.columns.push(col);
        }
        _ => out.push(IndexRow {
            database: db,
            table: tbl,
            name: idx,
            columns: vec![col],
            unique,
        }),
    }
}

/// How often each index has actually been read.
///
/// The ground truth for "nobody uses this": not an estimate, a counter. It
/// needs `performance_schema`, so this half of the inventory is the half that
/// can be refused.
pub(crate) async fn mysql_index_use(
    server: &ServerConn,
    dbs: &[String],
) -> Result<Vec<IndexUse>, String> {
    if dbs.is_empty() {
        return Ok(Vec::new());
    }
    let sql = index_use_sql(dbs.len());

    let rows = {
        let mut meta = server.mysql_meta().await?;
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        for db in dbs {
            q = q.bind(db);
        }
        q.fetch_all(&mut *meta).await.map_err(|e| friendly(&e))?
    };

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(IndexUse {
                database: r.try_get::<String, _>("db").ok()?,
                table: r.try_get::<String, _>("tbl").ok()?,
                name: r.try_get::<String, _>("idx").ok()?,
                reads: r
                    .try_get::<Option<u64>, _>("reads")
                    .ok()
                    .flatten()
                    .unwrap_or(0),
            })
        })
        .collect())
}

/// Every foreign key's child columns, in order.
///
/// Returned as [`IndexRow`] because that is what it is compared against, and
/// giving it its own near-identical struct would mean writing the prefix
/// comparison twice.
pub(crate) async fn mysql_foreign_keys(
    server: &ServerConn,
    dbs: &[String],
) -> Result<Vec<IndexRow>, String> {
    if dbs.is_empty() {
        return Ok(Vec::new());
    }
    let sql = foreign_keys_sql(dbs.len());

    let rows = {
        let mut meta = server.mysql_meta().await?;
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        for db in dbs {
            q = q.bind(db);
        }
        q.fetch_all(&mut *meta).await.map_err(|e| friendly(&e))?
    };

    let mut out: Vec<IndexRow> = Vec::new();
    for r in rows {
        // `KEY_COLUMN_USAGE.COLUMN_NAME` is never `NULL` -- a foreign key is
        // on columns, not expressions -- so a missing one here is a row that
        // did not decode and is still dropped.
        let (Ok(db), Ok(tbl), Ok(name), Ok(col)) = (
            r.try_get::<String, _>("db"),
            r.try_get::<String, _>("tbl"),
            r.try_get::<String, _>("name"),
            r.try_get::<String, _>("col"),
        ) else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.database == db && last.table == tbl && last.name == name => {
                last.columns.push(Some(col));
            }
            _ => out.push(IndexRow {
                database: db,
                table: tbl,
                name,
                columns: vec![Some(col)],
                unique: false,
            }),
        }
    }
    Ok(out)
}

/// The three reads the index inventory needs, in one trip to the engine.
///
/// One struct rather than three `Engine` methods: they are always wanted
/// together, and two of them are only meaningful compared against the third.
#[derive(Debug, Clone, Default)]
pub struct IndexScan {
    pub indexes: Vec<IndexRow>,
    pub uses: Vec<IndexUse>,
    /// `false` when the usage read did not answer, so "never read" is simply
    /// absent rather than wrongly empty. The structural faults still stand.
    pub usage_available: bool,
    /// **Why** it did not answer, when it did not.
    ///
    /// This was `Err(_)` -- the reason thrown away and the panel left saying
    /// "performance_schema refused" whatever had happened. A privilege wall,
    /// a deadline and a malformed query are three different things, and the
    /// third one is ours. Reporting all three as the first is how a bug in
    /// this module gets filed against somebody's database permissions.
    pub usage_refusal: Option<String>,
    pub foreign_keys: Vec<IndexRow>,
}

/// Read what indexes exist, which have been read, and which keys lack one.
///
/// The usage half is **tolerated separately**: a connection that can read
/// `information_schema` but not `performance_schema` still gets redundant
/// indexes and unindexed foreign keys, which is the whole argument for having
/// a structural tier at all.
pub(crate) async fn mysql_index_scan(
    server: &ServerConn,
    dbs: &[String],
) -> Result<IndexScan, String> {
    let indexes = mysql_indexes(server, dbs).await?;
    let foreign_keys = mysql_foreign_keys(server, dbs).await?;
    let (uses, usage_available, usage_refusal) = match mysql_index_use(server, dbs).await {
        Ok(u) => (u, true, None),
        Err(e) => (Vec::new(), false, Some(e)),
    };
    Ok(IndexScan {
        indexes,
        uses,
        usage_available,
        usage_refusal,
        foreign_keys,
    })
}

async fn show_status(meta: &mut sqlx::MySqlConnection, name: &'static str) -> Option<u64> {
    let sql = format!("SHOW GLOBAL STATUS LIKE '{name}'");
    // Safe to assert: `name` is a `&'static str` from this module's own call
    // sites, never anything a user typed.
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_one(&mut *meta)
        .await
        .ok()?;
    row.try_get::<String, _>("Value")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::TableRef;

    // ------------------------------------------------- the SQL this module sends
    //
    // Pure builders, so the parts that cannot be unit-tested against a server
    // are at least pinned here. Until the probe was run on 2026-11-05 none of
    // this SQL had been executed anywhere, and these three tests are what that
    // taught: the hint has to be first, the quantile has to be gated, and a
    // bound value has to have a placeholder waiting for it.

    /// **Every read in this module shares one connection with the schema tree,
    /// and that connection cannot be cancelled.** So each must carry its own
    /// deadline, and an optimizer hint only counts if it comes straight after
    /// `SELECT` -- anywhere else MySQL takes it for a comment and ignores it.
    #[test]
    fn every_shared_read_carries_its_deadline_in_the_only_place_it_works() {
        let reads = [
            ("digest, 8.0", digest_sql(true)),
            ("digest, pre-8.0", digest_sql(false)),
            ("indexes", indexes_sql(2)),
            ("index use", index_use_sql(2)),
            ("foreign keys", foreign_keys_sql(2)),
        ];
        let want = format!("SELECT {} ", deadline());
        for (name, sql) in reads {
            assert!(
                sql.starts_with(&want),
                "{name} has no deadline where MySQL would read one: {sql}"
            );
        }
        assert!(
            deadline().contains("MAX_EXECUTION_TIME"),
            "the deadline has to be the server's, not ours: a client-side one \
             abandons the shared connection, which is the thing being protected"
        );
    }

    /// `QUANTILE_95` does not exist before 8.0, and asking for a column that is
    /// not there fails the whole read rather than losing one value.
    #[test]
    fn the_quantile_is_asked_for_only_where_it_exists() {
        assert!(digest_sql(true).contains("QUANTILE_95 AS `q95`"));
        let old = digest_sql(false);
        assert!(!old.contains("QUANTILE_95"), "{old}");
        assert!(old.contains("NULL AS `q95`"), "{old}");
    }

    /// Every database name is bound, so every one needs a placeholder. A count
    /// that drifts binds a value to nothing and the server rejects the read.
    #[test]
    fn one_placeholder_per_database_in_every_inventory_read() {
        for n in [1_usize, 2, 7] {
            assert_eq!(indexes_sql(n).matches('?').count(), n);
            assert_eq!(index_use_sql(n).matches('?').count(), n);
            assert_eq!(foreign_keys_sql(n).matches('?').count(), n);
        }
        // The digest read has no predicate at all, so it binds nothing.
        assert_eq!(digest_sql(true).matches('?').count(), 0);
    }

    /// The panel has to be able to say *which half* is missing: a refused
    /// inventory beside a true workload is not a broken panel.
    #[test]
    fn a_deadline_on_the_inventory_explains_itself() {
        let hit = refusal_text(Some(DEADLINE_CODE), || "raw driver text".into());
        assert!(hit.contains("15 seconds"), "{hit}");
        assert!(
            hit.contains("workload above is unaffected"),
            "the point of the sentence is which half still holds: {hit}"
        );
        // Everything else is relayed untouched -- a privilege error is not a
        // timeout, and dressing it up as one would be a lie.
        assert_eq!(refusal_text(Some("1142"), || "raw".into()), "raw");
        assert_eq!(refusal_text(None, || "raw".into()), "raw");
        assert_eq!(
            inventory_refused(&sqlx::Error::RowNotFound),
            friendly(&sqlx::Error::RowNotFound)
        );
    }

    /// The sentence quotes the deadline, so it is derived from it.
    #[test]
    fn the_sentence_and_the_sql_quote_the_same_deadline() {
        assert!(deadline().contains(&DEADLINE_MS.to_string()));
        let text = refusal_text(Some(DEADLINE_CODE), || String::new());
        assert!(text.contains(&(DEADLINE_MS / 1000).to_string()), "{text}");
    }

    fn known() -> KnownTables {
        let tables = vec![
            TableRef {
                name: "Supplier".into(),
                kind: "BASE TABLE".into(),
            },
            TableRef {
                name: "acl_rule".into(),
                kind: "BASE TABLE".into(),
            },
            TableRef {
                name: "partysettings".into(),
                kind: "BASE TABLE".into(),
            },
            TableRef {
                name: "partydocuments".into(),
                kind: "BASE TABLE".into(),
            },
            TableRef {
                name: "backgroundservicehistory".into(),
                kind: "BASE TABLE".into(),
            },
            TableRef {
                name: "party_bouncing_view".into(),
                kind: "VIEW".into(),
            },
            TableRef {
                name: "get_worklist_clients".into(),
                kind: "VIEW".into(),
            },
        ];
        let schema = DbSchema {
            tables: Some(tables),
            ..Default::default()
        };
        known_tables(std::iter::once(&schema))
    }

    /// The real rows, as measured on 2026-11-04, with the statement text of
    /// `supplier-select` re-read on 2026-11-05. Times converted from the
    /// seconds the probe printed back into the picoseconds the table stores.
    ///
    /// The first probe truncated that statement mid-predicate and the stand-in
    /// written here read `OR supplier_master_id = ?`, which is sargable. The
    /// real one is not, and the fixture said so only after the whole 288-byte
    /// digest came back. Every verdict assertion about the worst shape on the
    /// server rested on the half of the predicate that had been cut off.
    fn uat() -> Vec<DigestRow> {
        let s = |secs: f64| (secs * 1e12) as u64;
        let ms = |millis: f64| (millis * 1e9) as u64;
        vec![
            DigestRow {
                digest: "supplier-select".into(),
                text: "SELECT `SupplierId` , `default_rate` AS `DefaultApr` , `include_in_plan` AS `IncludeInProgram` , `import_on_sync` AS `ImportOnPull` FROM `Supplier` WHERE `tenant_sub_id` = ? AND ( NAME = ? OR `JSON_CONTAINS` ( `LOWER` ( COALESCE ( ALIAS , ? ) ) , `JSON_QUOTE` ( `LOWER` (?) ) ) )".into(),
                schema: Some("appdata".into()),
                execs: 40_668_477,
                total_picos: s(285_820.11),
                avg_picos: ms(7.03),
                rows_sent: 41_708_854,
                rows_examined: 108_410_167_913,
                no_index: 1_766_585,
                scans: 1_766_585,
                first_seen: "2026-09-11 08:35:01".into(),
                first_seen_us: 1_789_000_101_788_702,
                ..Default::default()
            },
            DigestRow {
                digest: "casbin".into(),
                text: "SELECT `c` . `id` , `c` . `ptype` , `c` . `v0` , `c` . `v1` FROM `acl_rule` AS `c`".into(),
                schema: Some("appdata".into()),
                execs: 3_621,
                total_picos: s(32_200.70),
                avg_picos: ms(8_892.76),
                rows_sent: 1_511_063_716,
                rows_examined: 1_511_063_716,
                no_index: 3_621,
                scans: 3_621,
                ..Default::default()
            },
            DigestRow {
                digest: "cc-waiting".into(),
                text: "SELECT `cc` . `partyId` FROM `appdata` . `partysettings` `cc` WHERE `cc` . `tenantId` = ? AND `cc` . `isInSharktank` = ?".into(),
                schema: Some("appdata".into()),
                execs: 78_685,
                total_picos: s(22_953.68),
                avg_picos: ms(291.72),
                rows_sent: 14_687,
                rows_examined: 0,
                ..Default::default()
            },
            DigestRow {
                digest: "supplier-update-name".into(),
                text: "UPDATE `Supplier` SET NAME = ? , `last_updated_by` = ? WHERE `tenant_sub_id` = ? AND `supplier_master_id` = ?".into(),
                schema: Some("appdata".into()),
                execs: 40_666_497,
                total_picos: s(7_037.31),
                avg_picos: ms(0.17),
                rows_sent: 0,
                rows_examined: 40_903_221,
                first_seen: "2026-09-11 08:35:01".into(),
                first_seen_us: 1_789_000_101_648_379,
                ..Default::default()
            },
            DigestRow {
                digest: "supplier-update-address".into(),
                text: "UPDATE `Supplier` SET `Line1` = ? , `Line2` = ? , `City` = ? , `State` = ?".into(),
                schema: Some("appdata".into()),
                execs: 40_666_507,
                total_picos: s(5_517.53),
                avg_picos: ms(0.14),
                rows_sent: 0,
                rows_examined: 41_312_791,
                first_seen: "2026-09-11 08:35:01".into(),
                first_seen_us: 1_789_000_101_690_975,
                ..Default::default()
            },
            DigestRow {
                digest: "bouncing-view".into(),
                text: "SELECT `partyId` AS `PartyId` , `RuleId` , `PriorityOrder` , `LeadAge` FROM `party_bouncing_view` WHERE `tenantId` = ?".into(),
                schema: Some("appdata".into()),
                execs: 6_793,
                total_picos: s(3_033.26),
                avg_picos: ms(446.53),
                rows_sent: 15,
                rows_examined: 1_640_137_221,
                no_index: 6_793,
                scans: 1,
                ..Default::default()
            },
            DigestRow {
                digest: "partydocuments".into(),
                text: "SELECT `cd` . `Id` , `cd` . `PartyId` , `cd` . `SubId` FROM `partydocuments` `cd` WHERE `cd` . `PartyId` = ? ORDER BY `cd` . `Id` DESC".into(),
                schema: Some("appdata".into()),
                execs: 157_228,
                total_picos: s(591.25),
                avg_picos: ms(3.76),
                rows_sent: 18,
                rows_examined: 918_137_315,
                no_index: 157_228,
                scans: 157_228,
                sort_scans: 157_228,
                ..Default::default()
            },
            DigestRow {
                digest: "set-names".into(),
                text: "SET NAMES `utf8mb4`".into(),
                schema: Some("appdata".into()),
                execs: 128_121_716,
                total_picos: s(2_320.33),
                avg_picos: ms(0.02),
                ..Default::default()
            },
            DigestRow {
                digest: "show-status".into(),
                text: "SHOW STATUS LIKE ?".into(),
                execs: 14_573_912,
                total_picos: s(8_089.99),
                avg_picos: ms(0.56),
                rows_sent: 14_573_923,
                rows_examined: 14_573_923,
                no_index: 14_573_925,
                scans: 29_147_834,
                ..Default::default()
            },
            DigestRow {
                digest: "at-vars".into(),
                text: "SELECT @@`innodb_read_only` , @@`max_connections` , @@`autocommit` , @@`sql_mode`".into(),
                schema: Some("appdata".into()),
                execs: 126_412_282,
                total_picos: s(5_684.57),
                avg_picos: ms(0.04),
                rows_sent: 126_412_750,
                rows_examined: 126_412_559,
                ..Default::default()
            },
            DigestRow {
                digest: "heartbeat".into(),
                text: "INSERT INTO `mysql` . `rds_heartbeat2` ( `id` , VALUE ) VALUES (...) ON DUPLICATE KEY UPDATE VALUE = ?".into(),
                execs: 2_360_541,
                total_picos: s(3_598.10),
                avg_picos: ms(1.52),
                ..Default::default()
            },
            DigestRow {
                digest: "replica".into(),
                text: "SELECT `durable_lsn` , `current_read_point` , `server_id` FROM `information_schema` . `replica_host_status`".into(),
                execs: 4_991_955,
                total_picos: s(789.40),
                avg_picos: ms(0.16),
                rows_sent: 4_991_955,
                rows_examined: 4_991_955,
                no_index: 4_991_955,
                scans: 4_991_955,
                ..Default::default()
            },
        ]
    }

    fn verdict(digest: &str) -> Verdict {
        let k = known();
        let rows = uat();
        let row = rows.iter().find(|r| r.digest == digest).expect("digest");
        classify(row, &k).verdict
    }

    // ---- M4: the refusals, and that they come first -------------------

    #[test]
    fn a_query_that_returns_what_it_reads_is_not_offered_an_index() {
        // 1.51 B examined, 1.51 B returned, a scan on every execution.
        assert_eq!(verdict("casbin"), Verdict::NoIndexWillHelp);
        assert!(!verdict("casbin").proposes_an_index());
    }

    #[test]
    fn the_refusal_is_checked_before_the_proposal() {
        // `party_bouncing_view` satisfies the MissingIndex rule outright —
        // ratio 109,342,481 with `no_index` on every execution — so the only
        // thing standing between it and a proposed index is that the view
        // check runs first. Moving that check below MissingIndex makes this
        // fail, which is the point of it.
        let k = known();
        let rows = uat();
        let view = rows.iter().find(|r| r.digest == "bouncing-view").unwrap();
        assert!(view.ratio().unwrap() > RATIO_WANTS_INDEX);
        assert!(view.no_index > 0, "the MissingIndex trigger is present");
        assert_eq!(classify(view, &k).verdict, Verdict::ViewBlocksFilter);
    }

    #[test]
    fn a_view_is_never_offered_an_index() {
        // 1.64 B examined to return 15 — the worst ratio measured, and an
        // index on the name in the query cannot exist.
        assert_eq!(verdict("bouncing-view"), Verdict::ViewBlocksFilter);
        assert!(!verdict("bouncing-view").proposes_an_index());
    }

    #[test]
    fn zero_rows_examined_and_a_slow_average_is_waiting() {
        assert_eq!(verdict("cc-waiting"), Verdict::WaitingNotReading);
    }

    #[test]
    fn the_shapes_that_do_want_an_index_get_one() {
        assert_eq!(verdict("partydocuments"), Verdict::MissingIndex);
        assert!(verdict("partydocuments").proposes_an_index());
    }

    /// The shape this whole module was built to explain, and the one it got
    /// wrong. Every `MissingIndex` trigger is present -- the ratio, the
    /// executions that found no index -- and the answer is still no, because
    /// the predicate puts a function round a column.
    #[test]
    fn the_worst_shape_on_the_server_is_not_offered_an_index() {
        let row = uat()
            .into_iter()
            .find(|r| r.digest == "supplier-select")
            .expect("the fixture carries it");
        assert!(
            row.ratio().unwrap() > RATIO_WANTS_INDEX,
            "the ratio trigger"
        );
        assert!(row.no_index > 0, "the no-index trigger");

        assert_eq!(verdict("supplier-select"), Verdict::PredicateNotIndexable);
        assert!(!verdict("supplier-select").proposes_an_index());
    }

    // ---- the predicate detector ----------------------------------------

    /// The form a digest actually arrives in: the function names are
    /// backtick-quoted too, so a scanner that only looked for bare words
    /// would never fire on the real text.
    #[test]
    fn a_backticked_function_name_is_still_a_function() {
        assert!(wraps_a_column(
            "SELECT `a` FROM `t` WHERE `JSON_CONTAINS` ( `LOWER` ( COALESCE ( ALIAS , ? ) ) , ? )"
        ));
    }

    /// The distinction the whole rule rests on. Wrapping the *parameter* is
    /// free -- `col = LOWER(?)` uses an index on `col` perfectly well -- and
    /// a rule that could not tell the two apart would condemn half the
    /// queries on any server.
    #[test]
    fn wrapping_the_parameter_is_not_wrapping_the_column() {
        assert!(!wraps_a_column(
            "SELECT `a` FROM `t` WHERE `name` = `LOWER` (?)"
        ));
        assert!(!wraps_a_column(
            "SELECT `a` FROM `t` WHERE `name` = LOWER ( ? )"
        ));
        assert!(wraps_a_column(
            "SELECT `a` FROM `t` WHERE LOWER ( `name` ) = ?"
        ));
    }

    /// A function in the select list says nothing about whether the filter
    /// can use an index.
    #[test]
    fn only_the_where_clause_counts() {
        assert!(!wraps_a_column(
            "SELECT `LOWER` ( `name` ) FROM `t` WHERE `id` = ?"
        ));
        assert!(!wraps_a_column("SELECT `LOWER` ( `name` ) FROM `t`"));
    }

    /// A plain predicate must fall through, or the new verdict would swallow
    /// the one case that genuinely wants an index.
    #[test]
    fn a_sargable_predicate_is_left_alone() {
        assert!(!wraps_a_column(
            "SELECT `a` FROM `t` WHERE `sub_id` = ? AND ( `name` = ? OR `master_id` = ? )"
        ));
        assert!(!wraps_a_column("SELECT `a` FROM `t` WHERE `id` IN (?)"));
    }

    /// Digests are generated text, but they are text from a server and this
    /// scanner walks bytes. Truncation must not panic.
    #[test]
    fn a_half_a_statement_does_not_panic() {
        assert!(!wraps_a_column(
            "SELECT `a` FROM `t` WHERE `LOWER` ( `name`"
        ));
        assert!(!wraps_a_column("SELECT `a` FROM `t` WHERE `LOWER` ("));
        assert!(!wraps_a_column("WHERE"));
        assert!(!wraps_a_column(""));
        assert!(!wraps_a_column("SELECT `a` FROM `t` WHERE `unclosed"));
    }

    /// The rung sits inside the `MissingIndex` branch, so it can only ever
    /// change that one verdict. Everything the earlier rules claim is
    /// untouched by it -- which is the whole reason it was put there.
    #[test]
    fn the_new_rung_cannot_reach_any_other_verdict() {
        let k = known();
        let wrapped = " WHERE `LOWER` ( `name` ) = ?";

        for digest in ["casbin", "cc-waiting", "set-names", "bouncing-view"] {
            let before = verdict(digest);
            let mut row = uat()
                .into_iter()
                .find(|r| r.digest == digest)
                .expect("fixture");
            row.text.push_str(wrapped);
            assert_eq!(
                classify(&row, &k).verdict,
                before,
                "{digest} changed verdict because of a wrapped column"
            );
        }
    }

    // ---- M2: chatter and platform, classified not listed ---------------

    #[test]
    fn session_statements_are_chatter_without_being_named() {
        assert_eq!(verdict("set-names"), Verdict::DriverChatter);
    }

    #[test]
    fn show_is_chatter_even_though_it_scans_and_finds_no_index() {
        // 14.5 M executions with `no_index` higher than the execution count.
        // Treating it as an application shape would put it near the top of
        // the list it has no business being in.
        assert_eq!(verdict("show-status"), Verdict::DriverChatter);
    }

    #[test]
    fn a_select_of_nothing_but_variables_is_chatter() {
        assert_eq!(verdict("at-vars"), Verdict::DriverChatter);
    }

    #[test]
    fn a_real_query_mentioning_a_variable_is_not_chatter() {
        // The rule is "no FROM", not "contains @@", so this must survive.
        let k = known();
        let row = DigestRow {
            digest: "d".into(),
            text: "SELECT `SupplierId` , @@`sql_mode` FROM `Supplier` WHERE `tenant_sub_id` = ?".into(),
            execs: 10,
            rows_sent: 1,
            rows_examined: 1,
            ..Default::default()
        };
        assert_ne!(classify(&row, &k).verdict, Verdict::DriverChatter);
    }

    #[test]
    fn platform_housekeeping_is_classified_out() {
        assert_eq!(verdict("heartbeat"), Verdict::PlatformInternal);
        assert_eq!(verdict("replica"), Verdict::PlatformInternal);
    }

    #[test]
    fn classes_split_the_way_the_measurement_said() {
        let r = report(&uat(), &known());
        assert_eq!(r.chatter_shapes, 3, "SET NAMES, SHOW STATUS, the @@ select");
        assert_eq!(
            r.platform_shapes, 2,
            "rds_heartbeat2 and replica_host_status"
        );
        assert_eq!(r.application_shapes, 7);
        assert_eq!(r.total_shapes, 12);
    }

    // ---- M3: no table is named that the schema has no name for ---------

    #[test]
    fn only_tables_the_schema_knows_are_named() {
        let k = known();
        let rows = uat();
        for row in &rows {
            for t in classify(row, &k).tables {
                assert!(k.contains_key(&t), "{t} was reported without being known");
            }
        }
    }

    #[test]
    fn a_table_nobody_has_is_not_reported() {
        let k = known();
        let row = DigestRow {
            digest: "d".into(),
            text: "SELECT * FROM `a_table_that_does_not_exist` WHERE `x` = ?".into(),
            execs: 5,
            rows_sent: 1,
            rows_examined: 900_000,
            no_index: 5,
            ..Default::default()
        };
        let f = classify(&row, &k);
        assert!(f.tables.is_empty());
        // The ratio rules still work — they need no name at all.
        assert_eq!(f.verdict, Verdict::MissingIndex);
    }

    #[test]
    fn a_qualified_name_loses_its_database() {
        let k = known();
        let found = tables_named(
            "SELECT `cc` . `partyId` FROM `appdata` . `partysettings` `cc`",
            &k,
        );
        assert_eq!(found, vec!["partysettings"]);
    }

    #[test]
    fn a_derived_table_does_not_swallow_the_real_one() {
        let k = known();
        let found = tables_named(
            "SELECT COUNT ( * ) FROM ( SELECT `c` . `partyId` FROM `get_worklist_clients` `c` ) `x`",
            &k,
        );
        assert_eq!(found, vec!["get_worklist_clients"]);
    }

    #[test]
    fn a_join_names_both_sides() {
        let k = known();
        let found = tables_named(
            "SELECT * FROM `Supplier` `c1` JOIN `partysettings` `c2` ON `c1` . `x` = `c2` . `y`",
            &k,
        );
        assert_eq!(found.len(), 2);
        assert!(found.contains(&"supplier".to_string()));
        assert!(found.contains(&"partysettings".to_string()));
    }

    // ---- M5: a view is known without a round trip ----------------------

    #[test]
    fn view_ness_comes_from_the_schema_cache_alone() {
        let k = known();
        assert_eq!(k.get("party_bouncing_view"), Some(&true));
        assert_eq!(k.get("supplier"), Some(&false));
    }

    #[test]
    fn a_name_that_is_a_view_in_any_database_is_treated_as_one() {
        let a = DbSchema {
            tables: Some(vec![TableRef {
                name: "thing".into(),
                kind: "BASE TABLE".into(),
            }]),
            ..Default::default()
        };
        let b = DbSchema {
            tables: Some(vec![TableRef {
                name: "thing".into(),
                kind: "VIEW".into(),
            }]),
            ..Default::default()
        };
        let k = known_tables([&a, &b].into_iter());
        assert_eq!(
            k.get("thing"),
            Some(&true),
            "refusing is the safe direction"
        );
    }

    // ---- M6: the loop detector, both directions ------------------------

    #[test]
    fn the_supplier_triple_is_found() {
        let rows = uat();
        let k = known();
        let findings: Vec<Finding> = rows.iter().map(|r| classify(r, &k)).collect();
        let found = loops(&rows, &findings);

        let supplier = found
            .iter()
            .find(|l| l.table == "supplier")
            .expect("the Supplier loop");
        assert_eq!(supplier.digests.len(), 3);
        assert_eq!(supplier.execs_high, 40_668_477);
        assert_eq!(supplier.execs_low, 40_666_497);
        assert!(supplier.spread < 0.0001, "spread was {}", supplier.spread);
        assert!(supplier.first_seen_spread_us <= 140_323);
        // 79h 23m + 1h 57m + 1h 32m.
        assert!((supplier.total_secs - 298_374.95).abs() < 1.0);
    }

    #[test]
    fn two_unrelated_shapes_at_the_same_frequency_are_not_a_loop() {
        // The negative case. A detector with one real positive and no
        // negative has not been tested.
        let k = known();
        let rows = vec![
            DigestRow {
                digest: "a".into(),
                text: "SELECT `x` FROM `Supplier` WHERE `a` = ?".into(),
                execs: 1_000_000,
                rows_sent: 1_000_000,
                rows_examined: 1_000_000,
                first_seen_us: 1_000_000_000,
                ..Default::default()
            },
            DigestRow {
                digest: "b".into(),
                text: "SELECT `y` FROM `Supplier` WHERE `b` = ?".into(),
                execs: 1_000_100,
                rows_sent: 1_000_100,
                rows_examined: 1_000_100,
                first_seen_us: 1_000_000_500,
                ..Default::default()
            },
        ];
        let findings: Vec<Finding> = rows.iter().map(|r| classify(r, &k)).collect();
        assert!(
            loops(&rows, &findings).is_empty(),
            "two reads at one frequency are something on a timer, not a loop"
        );
    }

    #[test]
    fn a_write_far_from_the_read_in_time_is_not_a_loop() {
        let k = known();
        let rows = vec![
            DigestRow {
                digest: "a".into(),
                text: "SELECT `x` FROM `Supplier` WHERE `a` = ?".into(),
                execs: 1_000_000,
                rows_sent: 1_000_000,
                rows_examined: 1_000_000,
                first_seen_us: 1_000_000_000,
                ..Default::default()
            },
            DigestRow {
                digest: "b".into(),
                text: "UPDATE `Supplier` SET `y` = ? WHERE `b` = ?".into(),
                execs: 1_000_100,
                rows_examined: 1_000_100,
                // Ten seconds later: the same frequency, a different cause.
                first_seen_us: 1_010_000_000,
                ..Default::default()
            },
        ];
        let findings: Vec<Finding> = rows.iter().map(|r| classify(r, &k)).collect();
        assert!(loops(&rows, &findings).is_empty());
    }

    #[test]
    fn a_loop_member_with_a_better_verdict_keeps_it() {
        let rows = uat();
        let k = known();
        let r = report(&rows, &k);

        let select = r
            .shapes
            .iter()
            .find(|f| f.digest == "supplier-select")
            .unwrap();
        assert_eq!(
            select.verdict,
            Verdict::PredicateNotIndexable,
            "the read in a loop keeps its own verdict rather than becoming the loop"
        );

        let update = r
            .shapes
            .iter()
            .find(|f| f.digest == "supplier-update-name")
            .unwrap();
        assert_eq!(update.verdict, Verdict::RowByRowLoop);
    }

    // ---- M7: the proposal is priced ------------------------------------

    #[test]
    fn write_pressure_totals_the_two_updates() {
        let w = write_pressure(&uat(), &known());
        assert_eq!(w.get("supplier"), Some(&81_333_004));
    }

    #[test]
    fn platform_writes_are_not_charged_to_anybody() {
        let w = write_pressure(&uat(), &known());
        assert!(!w.contains_key("rds_heartbeat2"));
    }

    #[test]
    fn a_read_only_table_has_no_write_pressure() {
        let w = write_pressure(&uat(), &known());
        assert_eq!(w.get("acl_rule"), None);
    }

    // ---- the report ----------------------------------------------------

    #[test]
    fn shapes_arrive_most_expensive_first() {
        let r = report(&uat(), &known());
        let mut prev = f64::INFINITY;
        for s in &r.shapes {
            assert!(s.total_secs <= prev, "out of order at {}", s.digest);
            prev = s.total_secs;
        }
    }

    #[test]
    fn quantiles_are_reported_absent_when_the_server_has_none() {
        let r = report(&uat(), &known());
        assert!(!r.has_quantiles, "none of the measured rows carried p95");
    }

    // ---- the index inventory -------------------------------------------

    /// An index on named columns. `"?"` is a functional key part, which is
    /// what `STATISTICS` gives us for an expression: present, and unreadable.
    fn idx(table: &str, name: &str, cols: &[&str], unique: bool) -> IndexRow {
        IndexRow {
            database: "appdata".into(),
            table: table.into(),
            name: name.into(),
            columns: cols
                .iter()
                .map(|c| if *c == "?" { None } else { Some(c.to_string()) })
                .collect(),
            unique,
        }
    }

    // ---- functional key parts, as measured on 2026-11-06 --------------

    /// Whether the `AS` at `at` is a `CAST(... AS type)` target rather than
    /// an alias: the nearest enclosing unclosed paren belongs to a `CAST`.
    fn is_a_cast_target(sql: &str, at: usize) -> bool {
        let mut depth = 0i32;
        for (i, c) in sql[..at].char_indices().rev() {
            match c {
                ')' => depth += 1,
                '(' => {
                    if depth == 0 {
                        return sql[..i].trim_end().to_ascii_uppercase().ends_with("CAST");
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        false
    }

    /// Every alias in a generated query is quoted, and **no cast target is**.
    ///
    /// `COUNT_READ AS reads` was rejected by a live server on 2026-11-06.
    /// Whether `READS` is reserved on that exact version is not something this
    /// repo can check from vendored source -- but it does not need to be. A
    /// quoted alias is valid whatever the server's keyword list says, and this
    /// panel generates SQL for every MySQL it is pointed at, including
    /// versions whose reserved words it cannot know.
    ///
    /// **The second half of this assertion is why it exists.** The first pass
    /// at the fix quoted every ` AS x` in sight, which turned
    /// `CAST(... AS UNSIGNED)` into ``CAST(... AS `UNSIGNED`)`` -- invalid,
    /// in the one query the whole panel is built on. The test passed anyway,
    /// because it only asked whether a backtick followed. A test that checks
    /// one direction of a rule will wave through the mistake that breaks the
    /// other.
    #[test]
    fn every_generated_alias_is_quoted_and_no_cast_target_is() {
        for (what, sql) in [
            ("digest", digest_sql(true)),
            ("digest, pre-8.0", digest_sql(false)),
            ("indexes", indexes_sql(2)),
            ("index use", index_use_sql(2)),
            ("foreign keys", foreign_keys_sql(2)),
        ] {
            let mut from = 0usize;
            while let Some(rel) = sql[from..].find(" AS ") {
                let at = from + rel;
                let after = &sql[at + " AS ".len()..];
                let shown = &after[..after.len().min(24)];
                if is_a_cast_target(&sql, at + 1) {
                    assert!(
                        !after.starts_with('`'),
                        "{what}: quoted cast target at {shown:?} -- a type is not an identifier"
                    );
                } else {
                    assert!(
                        after.starts_with('`'),
                        "{what}: unquoted alias at {shown:?}"
                    );
                }
                from = at + " AS ".len();
            }
        }
    }

    /// The reason a usage read failed is carried, not replaced by a guess.
    ///
    /// `mysql_index_scan` matched `Err(_)` and the panel then told the user
    /// "performance_schema refused" whatever had actually happened --
    /// including a query this module had got wrong. The struct has somewhere
    /// to put the truth now, and the default is "nothing went wrong".
    #[test]
    fn a_usage_refusal_has_somewhere_to_say_why() {
        let clean = IndexScan::default();
        assert!(clean.usage_refusal.is_none());

        let refused = IndexScan {
            usage_available: false,
            usage_refusal: Some("You have no SELECT privilege".into()),
            ..Default::default()
        };
        assert!(!refused.usage_available);
        assert_eq!(
            refused.usage_refusal.as_deref(),
            Some("You have no SELECT privilege")
        );
    }

    /// A functional key part keeps its place in the index.
    ///
    /// The bug this guards against was one decode: `COLUMN_NAME` read as a
    /// `String` when it is `NULL`, so the whole row failed and the part was
    /// dropped without a word. The index then arrived narrower than it is.
    #[test]
    fn a_null_column_name_is_a_key_part_and_not_a_lost_row() {
        let mut out = Vec::new();
        let part = |out: &mut Vec<IndexRow>, idx: &str, col: Option<&str>| {
            fold_part(
                out,
                "appdata".into(),
                "external_client".into(),
                idx.into(),
                col.map(str::to_string),
                true,
            )
        };
        part(
            &mut out,
            "uq_external_client_sc_external_id",
            Some("service_org_id"),
        );
        part(&mut out, "uq_external_client_sc_external_id", None);

        assert_eq!(out.len(), 1, "one index, two key parts");
        assert_eq!(
            out[0].columns,
            vec![Some("service_org_id".to_string()), None],
            "the expression part was dropped instead of being kept as unreadable"
        );
    }

    /// The five findings that were being lost.
    ///
    /// `external_client` really carries a one-column non-unique index on
    /// `service_org_id` **and** a unique index on
    /// `(service_org_id, <expression>)`. The narrow one is redundant: the
    /// unique one can serve it. While the expression part was dropped on
    /// decode both indexes looked one column wide, so neither was a strict
    /// prefix of the other and nothing was reported.
    #[test]
    fn an_index_covered_by_a_functional_one_is_still_redundant() {
        let all = vec![
            idx(
                "external_client",
                "idx_external_client_service_org_id",
                &["service_org_id"],
                false,
            ),
            idx(
                "external_client",
                "uq_external_client_sc_external_id",
                &["service_org_id", "?"],
                true,
            ),
        ];
        let found = redundant(&all);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0.name, "idx_external_client_service_org_id");
        assert_eq!(found[0].1, "uq_external_client_sc_external_id");
    }

    /// And the asymmetry that keeps it honest: an expression is opaque, so an
    /// index that *contains* one is never proposed for dropping. Two opaque
    /// parts are not known to be equal, and `STATISTICS` does not report what
    /// either of them is.
    #[test]
    fn an_index_with_an_expression_is_never_called_redundant() {
        let all = vec![
            idx("t", "narrow", &["a", "?"], false),
            idx("t", "wide", &["a", "?", "b"], false),
        ];
        assert!(
            redundant(&all).is_empty(),
            "an unreadable key part was treated as equal to another one"
        );
    }

    /// `uq_reconciliation_config_scope` is `(<expr>, <expr>, entity_type)` on
    /// both servers. Dropping the expressions left it reading as an index on
    /// `(entity_type)` -- which would have counted as support for a foreign
    /// key on that column that it does not serve.
    #[test]
    fn a_leading_expression_supports_no_foreign_key() {
        let indexes = vec![idx(
            "reconciliation_config",
            "uq_reconciliation_config_scope",
            &["?", "?", "entity_type"],
            true,
        )];
        let keys = vec![idx(
            "reconciliation_config",
            "fk_entity",
            &["entity_type"],
            false,
        )];

        let unsupported = unindexed_foreign_keys(&keys, &indexes);
        assert_eq!(
            unsupported.len(),
            1,
            "an expression was accepted as support for a column"
        );

        // And it is supported once a real index on it exists.
        let mut with_support = indexes.clone();
        with_support.push(idx(
            "reconciliation_config",
            "idx_entity",
            &["entity_type"],
            false,
        ));
        assert!(unindexed_foreign_keys(&keys, &with_support).is_empty());
    }

    /// What the panel shows. A blank would read as "this index has no
    /// columns", which is a different and wronger claim than "this part is an
    /// expression we cannot see".
    #[test]
    fn an_expression_is_named_in_the_finding() {
        let cols = idx("t", "i", &["a", "?"], false).columns;
        assert_eq!(shown(&cols), vec!["a", "(expression)"]);
    }

    #[test]
    fn a_prefix_index_is_redundant() {
        let all = vec![
            idx("Supplier", "idx_owner", &["tenant_sub_id"], false),
            idx(
                "Supplier",
                "idx_owner_name",
                &["tenant_sub_id", "Name"],
                false,
            ),
        ];
        let found = redundant(&all);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.name, "idx_owner");
        assert_eq!(found[0].1, "idx_owner_name");
    }

    #[test]
    fn a_unique_index_is_not_redundant_against_a_plain_one() {
        // Dropping it changes what the table *accepts*, which is not a
        // question about access paths at all.
        let all = vec![
            idx("Supplier", "uq_master", &["supplier_master_id"], true),
            idx(
                "Supplier",
                "idx_master_owner",
                &["supplier_master_id", "tenant_sub_id"],
                false,
            ),
        ];
        assert!(
            redundant(&all).is_empty(),
            "a constraint must not be dropped for being covered"
        );
    }

    #[test]
    fn a_differently_ordered_index_is_not_a_prefix() {
        let all = vec![
            idx("Supplier", "idx_a_b", &["a", "b"], false),
            idx("Supplier", "idx_b_a_c", &["b", "a", "c"], false),
        ];
        assert!(redundant(&all).is_empty());
    }

    #[test]
    fn the_primary_key_is_never_proposed_for_dropping() {
        let all = vec![
            idx("Supplier", "PRIMARY", &["SupplierId"], true),
            idx("Supplier", "idx_id_name", &["SupplierId", "Name"], false),
        ];
        assert!(redundant(&all).is_empty());
    }

    #[test]
    fn an_index_on_another_table_is_not_a_prefix_of_this_one() {
        let all = vec![
            idx("Supplier", "idx_a", &["a"], false),
            idx("partysettings", "idx_a_b", &["a", "b"], false),
        ];
        assert!(redundant(&all).is_empty());
    }

    #[test]
    fn a_foreign_key_with_a_leading_prefix_index_is_supported() {
        let keys = vec![idx("partydocuments", "fk_client", &["PartyId"], false)];
        let indexes = vec![idx(
            "partydocuments",
            "idx_client_type",
            &["PartyId", "Type"],
            false,
        )];
        assert!(unindexed_foreign_keys(&keys, &indexes).is_empty());
    }

    #[test]
    fn a_foreign_key_indexed_in_the_wrong_position_is_not_supported() {
        // `(Type, PartyId)` cannot answer "which children point at this
        // parent" without reading all of it.
        let keys = vec![idx("partydocuments", "fk_client", &["PartyId"], false)];
        let indexes = vec![idx(
            "partydocuments",
            "idx_type_client",
            &["Type", "PartyId"],
            false,
        )];
        assert_eq!(unindexed_foreign_keys(&keys, &indexes).len(), 1);
    }

    #[test]
    fn a_never_read_index_is_priced_by_the_writes_on_its_table() {
        let indexes = vec![idx("Supplier", "idx_unused", &["AltContact"], false)];
        let uses = vec![IndexUse {
            database: "appdata".into(),
            table: "Supplier".into(),
            name: "idx_unused".into(),
            reads: 0,
        }];
        let writes = write_pressure(&uat(), &known());
        let found = index_findings(&indexes, &uses, &[], &writes);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fault, IndexFault::NeverRead);
        assert_eq!(
            found[0].writes,
            Some(81_333_004),
            "an unused index on a table taking 81 M writes is the expensive kind"
        );
        assert_eq!(
            found[0].statement,
            "DROP INDEX `idx_unused` ON `appdata`.`Supplier`;"
        );
    }

    #[test]
    fn an_index_that_has_been_read_is_left_alone() {
        let uses = vec![IndexUse {
            database: "appdata".into(),
            table: "Supplier".into(),
            name: "idx_used".into(),
            reads: 1,
        }];
        assert!(index_findings(&[], &uses, &[], &HashMap::new()).is_empty());
    }

    #[test]
    fn the_primary_key_is_never_reported_as_unread() {
        // It is read by every lookup that goes through the clustered index,
        // which `COUNT_READ` on the index itself does not always show.
        let uses = vec![IndexUse {
            database: "appdata".into(),
            table: "Supplier".into(),
            name: "PRIMARY".into(),
            reads: 0,
        }];
        assert!(index_findings(&[], &uses, &[], &HashMap::new()).is_empty());
    }

    /// The split and the findings apply the same rule, so they must agree.
    ///
    /// This is the test that matters about [`index_usage_split`]: its
    /// predicate is a second copy of the one in [`index_findings`], and a bar
    /// whose "never read" segment disagreed with the number of never-read
    /// findings printed beside it would be worse than no bar. Asserted
    /// against the findings rather than against a literal, so the two cannot
    /// drift apart without this failing.
    #[test]
    fn the_split_agrees_with_the_findings_it_sits_above() {
        let use_row = |table: &str, name: &str, reads: u64| IndexUse {
            database: "appdata".into(),
            table: table.into(),
            name: name.into(),
            reads,
        };
        let uses = vec![
            use_row("Supplier", "PRIMARY", 0),
            use_row("Supplier", "ix_owner", 0),
            use_row("Supplier", "ix_name", 9_912),
            use_row("Payment", "PRIMARY", 4_410),
            use_row("Payment", "ix_stale", 0),
        ];

        let split = index_usage_split(&uses);
        assert_eq!(split.seen, 5);
        // Both PRIMARYs, whether or not the counter says they were read:
        // they are skipped by the rule, which is a different fact from the
        // data, and they are their own part for that reason.
        assert_eq!(split.primary_keys, 2);
        assert_eq!(split.read, 1);
        assert_eq!(split.unread, 2);
        assert_eq!(split.read + split.unread + split.primary_keys, split.seen);

        let unread = index_findings(&[], &uses, &[], &HashMap::new())
            .iter()
            .filter(|f| f.fault == IndexFault::NeverRead)
            .count();
        assert_eq!(split.unread as usize, unread);
    }

    /// Nothing read, nothing to divide -- and no division by it either.
    #[test]
    fn an_empty_usage_read_splits_into_nothing() {
        assert_eq!(index_usage_split(&[]), IndexUsageSplit::default());
    }

    #[test]
    fn an_unindexed_foreign_key_proposes_an_index_not_a_drop() {
        let keys = vec![idx("partydocuments", "fk_client", &["PartyId"], false)];
        let found = index_findings(&[], &[], &keys, &HashMap::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fault, IndexFault::UnindexedForeignKey);
        assert!(found[0].statement.starts_with("CREATE INDEX"));
        assert!(found[0].statement.contains("`PartyId`"));
    }

    #[test]
    fn a_backtick_in_a_name_is_doubled_rather_than_breaking_out() {
        let uses = vec![IndexUse {
            database: "db".into(),
            table: "t`x".into(),
            name: "i".into(),
            reads: 0,
        }];
        let found = index_findings(&[], &uses, &[], &HashMap::new());
        assert_eq!(found[0].statement, "DROP INDEX `i` ON `db`.`t``x`;");
    }

    // ---- M10: an empty table is diagnosed, not shrugged at -------------

    fn working() -> Readings {
        Readings {
            enabled: Some(true),
            digest_consumer: Some(true),
            statement_instruments: Some(192),
            uptime_secs: Some(4_721_760),
            digests_lost: Some(0),
            digests_present: Some(3_412),
        }
    }

    #[test]
    fn a_working_server_is_usable() {
        let d = diagnose(&working());
        assert!(d.usable);
    }

    #[test]
    fn switched_off_is_told_apart_from_not_allowed() {
        let off = Readings {
            enabled: Some(false),
            ..Default::default()
        };
        let denied = Readings {
            enabled: Some(true),
            ..Default::default()
        };
        assert!(!diagnose(&off).usable);
        assert!(!diagnose(&denied).usable);
        assert_ne!(
            diagnose(&off).headline,
            diagnose(&denied).headline,
            "a restart and a grant are different fixes and must read differently"
        );
        assert!(diagnose(&denied).detail.contains("GRANT SELECT"));
    }

    #[test]
    fn being_switched_off_outranks_every_other_rung() {
        // Everything below is also wrong, but saying "the consumer is off" to
        // somebody whose instrumentation is not running at all sends them to
        // the wrong setting.
        let r = Readings {
            enabled: Some(false),
            digest_consumer: Some(false),
            statement_instruments: Some(0),
            digests_present: Some(0),
            ..Default::default()
        };
        assert!(diagnose(&r).headline.contains("switched off"));
    }

    #[test]
    fn the_consumer_and_the_instruments_are_different_sentences() {
        let consumer = Readings {
            digest_consumer: Some(false),
            digests_present: Some(0),
            ..working()
        };
        let instruments = Readings {
            statement_instruments: Some(0),
            digests_present: Some(0),
            ..working()
        };
        assert!(diagnose(&consumer).headline.contains("consumer"));
        assert!(diagnose(&instruments).headline.contains("instrumentation"));
    }

    #[test]
    fn an_empty_table_on_a_fresh_server_blames_the_restart() {
        let fresh = Readings {
            digests_present: Some(0),
            uptime_secs: Some(600),
            ..working()
        };
        let old = Readings {
            digests_present: Some(0),
            uptime_secs: Some(4_721_760),
            ..working()
        };
        assert!(diagnose(&fresh).headline.contains("reset"));
        assert!(!diagnose(&old).headline.contains("reset"));
        assert_ne!(diagnose(&fresh).headline, diagnose(&old).headline);
    }

    #[test]
    fn an_overflowed_ring_is_usable_but_says_it_is_a_sample() {
        let lost = Readings {
            digests_lost: Some(14_220),
            ..working()
        };
        let d = diagnose(&lost);
        assert!(d.usable, "a sample is still worth reading");
        assert!(d.headline.contains("sample"));
    }

    #[test]
    fn no_diagnosis_ever_says_no_data() {
        let cases = [
            Readings::default(),
            working(),
            Readings {
                enabled: Some(false),
                ..Default::default()
            },
            Readings {
                digests_present: Some(0),
                ..working()
            },
            Readings {
                digest_consumer: Some(false),
                digests_present: Some(0),
                ..working()
            },
            Readings {
                statement_instruments: Some(0),
                digests_present: Some(0),
                ..working()
            },
            Readings {
                digests_lost: Some(1),
                ..working()
            },
        ];
        for r in cases {
            let d = diagnose(&r);
            assert!(!d.headline.is_empty(), "every rung says something");
            assert!(!d.detail.is_empty(), "and says what to do about it");
            let lower = d.headline.to_lowercase();
            assert!(
                !lower.contains("no data") && !lower.contains("nothing to show"),
                "shrugging is the failure mode this ladder exists to prevent: {lower}"
            );
        }
    }

    #[test]
    fn an_empty_ring_is_an_empty_report_and_not_a_panic() {
        let r = report(&[], &known());
        assert_eq!(r.total_shapes, 0);
        assert!(r.shapes.is_empty());
        assert!(r.loops.is_empty());
    }

    #[test]
    fn a_zero_execution_shape_is_unclassified_rather_than_divided_by() {
        let k = known();
        let row = DigestRow {
            digest: "d".into(),
            text: "SELECT * FROM `Supplier`".into(),
            ..Default::default()
        };
        assert_eq!(classify(&row, &k).verdict, Verdict::Unclassified);
    }

    #[test]
    fn an_update_examining_one_row_per_execution_is_not_the_worst_shape() {
        // `rows_sent` is zero for a write, so a naive ratio would be a
        // division by zero and sort to the top.
        let k = known();
        let row = DigestRow {
            digest: "d".into(),
            text: "UPDATE `Supplier` SET `x` = ? WHERE `id` = ?".into(),
            execs: 1_000,
            rows_sent: 0,
            rows_examined: 1_000,
            ..Default::default()
        };
        let f = classify(&row, &k);
        assert_eq!(f.ratio, None);
        assert_eq!(f.verdict, Verdict::Unclassified);
    }
}
