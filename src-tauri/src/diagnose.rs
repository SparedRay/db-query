//! Diagnose one statement: the plan it intends, the plan it actually ran, what
//! that cost the session, and a verdict on each of those facts.
//!
//! Stage 31's dock answers *which statements cost the server its time*. This
//! answers *why this one*, for a statement in front of you.
//!
//! **Three facts about the server shaped this module** (MySQL 8.0/8.4 manual,
//! read 2026-11-11):
//!
//! 1. `EXPLAIN ANALYZE` **executes the statement**, and it accepts multi-table
//!    `UPDATE` and `DELETE` as well as `SELECT`. So the gate here is
//!    row-returning-or-nothing, decided before a connection is touched. A
//!    toggle that ran a `DELETE` because the user was "only diagnosing" is the
//!    worst bug this app could ship.
//! 2. Its timings are **per loop and cumulative**: `actual time=0.05..268` is
//!    one iteration's first-row and last-row time, children included. A node's
//!    total is `last × loops`, and its own cost is that minus its children's.
//!    Reading the printed numbers as self time makes the root look like the
//!    problem every time.
//! 3. The tree carries **no counters**. Rows read, temporary tables on disk and
//!    sort merge passes come from a `SHOW SESSION STATUS` delta either side of
//!    the run, which is attributable because every tab holds its own
//!    connection.
//!
//! `EXPLAIN FORMAT=TREE` (8.0.16+) and `EXPLAIN ANALYZE` (8.0.18+) print the
//! same grammar — the second one adds an `(actual …)` group — so one parser
//! reads both, and the free plan is the fallback when there is no measurement
//! to be had.

use serde::Serialize;

use crate::exec::{classify, returns_rows};
use crate::split;

/// Where a plan came from, so the view never implies a measurement it does not
/// have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanSource {
    /// `EXPLAIN FORMAT=TREE` — estimates, no execution.
    Tree,
    /// `EXPLAIN ANALYZE` — the statement ran and these are its own numbers.
    Analyze,
    /// `EXPLAIN` in its tabular form, for a server with no tree at all.
    Classic,
}

/// One line of the plan.
///
/// `est_rows` and `actual_rows` sit side by side because **the gap between them
/// is the finding**: a plan chosen for 11,920 rows is the wrong plan for
/// 118,944, and that is visible in no other way.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    /// Indentation depth, 0 for the root.
    pub depth: usize,
    /// The operation, up to the first colon: `Table scan on Supplier`.
    pub op: String,
    /// Everything after it: the predicate, the sort keys, the index name.
    pub detail: String,
    pub est_rows: Option<f64>,
    pub cost: Option<f64>,
    pub actual_rows: Option<f64>,
    pub loops: Option<f64>,
    /// Last-row time for one loop, as printed.
    pub time_ms: Option<f64>,
    /// `time_ms × loops`, children included.
    pub total_ms: Option<f64>,
    /// `total_ms` minus the children's totals. What this node itself cost.
    pub self_ms: Option<f64>,
    /// The server printed `(never executed)`.
    pub never_executed: bool,
    /// The line could not be read. Its text is in `op` and nothing else is set.
    pub unparsed: bool,
}

/// One session counter that moved while the statement ran.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counter {
    pub name: String,
    pub delta: i64,
    /// True where a non-zero value is itself the finding.
    pub notable: bool,
}

/// How sure a finding is of itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Costs real time and has a known remedy.
    High,
    /// Worth knowing, may be the cause.
    Medium,
    /// A fact about the run, not a problem.
    Note,
}

/// A verdict, the evidence under it, and — only where the evidence supports one
/// — something to try.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// A stable key the UI styles and the tests assert on.
    pub code: String,
    pub title: String,
    pub severity: Severity,
    /// What the server said, in the finding's own words.
    pub evidence: String,
    /// SQL to put in the editor. Never run by the app.
    pub suggestion: Option<String>,
}

/// Everything one diagnosis produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementDiagnosis {
    /// The statement as written, for display.
    pub statement: String,
    /// The statement with its literals replaced, for the assistant.
    pub normalised: String,
    pub source: PlanSource,
    pub plan: Vec<PlanNode>,
    /// The classic tabular `EXPLAIN`, when that is all there was.
    pub classic: Vec<Vec<Option<String>>>,
    pub classic_columns: Vec<String>,
    pub counters: Vec<Counter>,
    pub findings: Vec<Finding>,
    /// Wall time of the measured run, where there was one.
    pub measured_ms: Option<u64>,
    /// Rows the statement would have returned.
    pub rows_sent: Option<f64>,
    /// What this diagnosis could not do, and why. Shown, never swallowed.
    pub notes: Vec<String>,
}

/// Why a statement cannot be diagnosed.
///
/// Returned rather than silently skipped: the control said *Diagnose*, so
/// something has to come back saying it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not row-returning. `EXPLAIN ANALYZE` would execute it.
    Writes,
    /// Nothing to diagnose.
    Empty,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::Writes => "Only a row-returning statement can be diagnosed. \
                 EXPLAIN ANALYZE runs what it is given, including UPDATE and DELETE, \
                 so this is refused rather than measured."
                .into(),
            Refusal::Empty => "There is no statement here to diagnose.".into(),
        }
    }
}

/// The gate, decided from the text alone — before a connection is touched.
pub fn may_diagnose(sql: &str) -> Result<(), Refusal> {
    if split::first_code_byte(sql).is_none() {
        return Err(Refusal::Empty);
    }
    if returns_rows(classify(sql)) {
        Ok(())
    } else {
        Err(Refusal::Writes)
    }
}

// --------------------------------------------------------------- the parser

/// One indentation step in MySQL's tree output.
const INDENT: usize = 4;

/// Read `EXPLAIN FORMAT=TREE` or `EXPLAIN ANALYZE` output into nodes.
///
/// **A line it cannot read becomes a node carrying its text**, not an error.
/// This output is documentation rather than an interface and it moves between
/// versions; a diagnosis with one opaque line is still useful, and one that
/// refuses to open because of it is not.
pub fn parse_tree(text: &str) -> Vec<PlanNode> {
    let mut nodes: Vec<PlanNode> = Vec::new();

    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let body = raw.trim_start();

        // Continuation of a wrapped line: no arrow, so it belongs to the node
        // above rather than being a node of its own.
        let Some(rest) = body.strip_prefix("-> ") else {
            if let Some(last) = nodes.last_mut() {
                if !last.detail.is_empty() {
                    last.detail.push(' ');
                }
                last.detail.push_str(body);
            }
            continue;
        };

        nodes.push(parse_line(indent / INDENT, rest));
    }

    attribute_time(&mut nodes);
    nodes
}

/// One `-> …` line.
fn parse_line(depth: usize, rest: &str) -> PlanNode {
    let mut node = PlanNode {
        depth,
        op: String::new(),
        detail: String::new(),
        est_rows: None,
        cost: None,
        actual_rows: None,
        loops: None,
        time_ms: None,
        total_ms: None,
        self_ms: None,
        never_executed: false,
        unparsed: false,
    };

    // The trailing `(…)` groups are the figures; everything before the first
    // one is the label. Scanned from the right, because a predicate contains
    // parentheses of its own and the label must keep them.
    let (label, groups) = split_groups(rest);

    let label = label.trim();
    match label.split_once(':') {
        Some((op, detail)) => {
            node.op = op.trim().to_string();
            node.detail = detail.trim().to_string();
        }
        None => node.op = label.to_string(),
    }

    if node.op.is_empty() {
        node.op = rest.trim().to_string();
        node.unparsed = true;
        return node;
    }

    for group in groups {
        let g = group.trim();
        if g == "never executed" {
            node.never_executed = true;
            continue;
        }
        if let Some(actual) = g.strip_prefix("actual ") {
            read_actual(actual, &mut node);
        } else if g.starts_with("cost=") || g.starts_with("rows=") {
            read_estimate(g, &mut node);
        }
    }

    node
}

/// Split a line into its label and its trailing parenthesised groups.
///
/// Only groups that sit at the end of the line, separated by spaces, count —
/// so `Filter: ((a = 1) or (b = 2))  (cost=7121 rows=11920)` keeps its
/// parentheses in the label and yields one group.
fn split_groups(rest: &str) -> (&str, Vec<&str>) {
    let bytes = rest.as_bytes();
    let mut groups: Vec<&str> = Vec::new();
    let mut end = rest.len();

    loop {
        let trimmed_end = rest[..end].trim_end().len();
        if trimmed_end == 0 || bytes[trimmed_end - 1] != b')' {
            break;
        }
        // Walk back to this group's own opening parenthesis.
        let mut depth = 0usize;
        let mut open = None;
        for i in (0..trimmed_end).rev() {
            match bytes[i] {
                b')' => depth += 1,
                b'(' => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(open) = open else { break };
        // A group is figures, not predicate text. `cost=`, `rows=`, `actual `
        // and `never executed` are the whole vocabulary MySQL prints here.
        let inner = &rest[open + 1..trimmed_end - 1];
        let is_figures = inner.starts_with("cost=")
            || inner.starts_with("rows=")
            || inner.starts_with("actual ")
            || inner == "never executed";
        if !is_figures {
            break;
        }
        groups.push(inner);
        end = open;
    }

    groups.reverse();
    (&rest[..end], groups)
}

/// `cost=48913 rows=119204`
fn read_estimate(group: &str, node: &mut PlanNode) {
    for field in group.split_whitespace() {
        if let Some(v) = field.strip_prefix("cost=") {
            node.cost = parse_num(v);
        } else if let Some(v) = field.strip_prefix("rows=") {
            node.est_rows = parse_num(v);
        }
    }
}

/// `time=0.05..268 rows=118944 loops=1`, with `actual ` already stripped.
fn read_actual(group: &str, node: &mut PlanNode) {
    for field in group.split_whitespace() {
        if let Some(v) = field.strip_prefix("time=") {
            // first..last — the last-row time is the one that bounds the node.
            node.time_ms = match v.split_once("..") {
                Some((_, last)) => parse_num(last),
                None => parse_num(v),
            };
        } else if let Some(v) = field.strip_prefix("rows=") {
            node.actual_rows = parse_num(v);
        } else if let Some(v) = field.strip_prefix("loops=") {
            node.loops = parse_num(v);
        }
    }
}

fn parse_num(s: &str) -> Option<f64> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == 'e' || *c == 'E')
        .collect();
    cleaned.parse::<f64>().ok()
}

/// Fill in `total_ms` and `self_ms`.
///
/// **This is the subtlety the whole view rests on.** The printed time is one
/// loop's, children included, so a node's total is `time × loops` and its own
/// cost is that minus the totals of the children directly under it. A node
/// whose children claim more time than it does lands at zero rather than
/// negative — rounding in the server's own output, not an error here.
fn attribute_time(nodes: &mut [PlanNode]) {
    for i in 0..nodes.len() {
        nodes[i].total_ms = match (nodes[i].time_ms, nodes[i].loops) {
            (Some(t), Some(l)) => Some(t * l),
            (Some(t), None) => Some(t),
            _ => None,
        };
    }

    for i in 0..nodes.len() {
        let Some(total) = nodes[i].total_ms else {
            continue;
        };
        let mut children = 0.0;
        let depth = nodes[i].depth;
        for j in i + 1..nodes.len() {
            if nodes[j].depth <= depth {
                break;
            }
            if nodes[j].depth == depth + 1 {
                children += nodes[j].total_ms.unwrap_or(0.0);
            }
        }
        nodes[i].self_ms = Some((total - children).max(0.0));
    }
}

// ------------------------------------------------------------- normalising

/// The statement with its literals replaced by `?` and its comments dropped.
///
/// **The assistant is an external API and this is real SQL.** The performance
/// dock can hand over a digest because a digest never held a value; a statement
/// being diagnosed holds every value the person typed. So this is what travels
/// by default, and sending the statement as written is a separate, deliberate
/// click.
///
/// Byte offsets come from [`split::mask_noncode`], which blanks literals,
/// quoted identifiers and comments while keeping every offset — so a `--` in a
/// string cannot start a comment here and a quote in a comment cannot open a
/// literal.
pub fn normalise(sql: &str) -> String {
    let mask = split::mask_noncode(sql);
    let src = sql.as_bytes();
    let msk = mask.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;

    while i < src.len() {
        let blanked = msk[i] == b' ' && !src[i].is_ascii_whitespace();
        if blanked {
            let start = i;
            while i < src.len() && msk[i] == b' ' && !src[i].is_ascii_whitespace() {
                i += 1;
            }
            match src[start] {
                // A string literal, or a date in quotes. One placeholder.
                b'\'' | b'"' => out.push('?'),
                // A quoted identifier is a name, not a value: keep it.
                b'`' => out.push_str(&sql[start..i]),
                // A comment. Dropped — it can hold anything at all.
                _ => out.push(' '),
            }
            continue;
        }

        // A number in code position is a literal too.
        if src[i].is_ascii_digit() && !preceded_by_ident_char(src, i) {
            while i < src.len()
                && (src[i].is_ascii_digit() || src[i] == b'.' || src[i] == b'_')
                && msk[i] != b' '
            {
                i += 1;
            }
            out.push('?');
            continue;
        }

        out.push(src[i] as char);
        i += 1;
    }

    collapse_space(&out)
}

/// `t1` is an identifier; the `1` in it is not a literal.
fn preceded_by_ident_char(src: &[u8], i: usize) -> bool {
    i > 0 && (src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_' || src[i - 1] == b'$')
}

fn collapse_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        let is_space = ch == ' ' || ch == '\t';
        if is_space {
            if !last_space {
                out.push(' ');
            }
        } else if ch == '\n' {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
        last_space = is_space;
    }
    out.trim().to_string()
}

// --------------------------------------------------------------- counters

/// The session counters worth reading, and whether a non-zero delta is itself
/// a finding.
///
/// Deliberately short. Every one of these answers a question the plan tree
/// cannot, and a counter nobody can act on is noise in a panel people are
/// meant to read quickly.
pub const WATCHED: &[(&str, bool)] = &[
    // Rows walked in table order: a full scan's fingerprint.
    ("Handler_read_rnd_next", true),
    // Rows walked along an index.
    ("Handler_read_next", false),
    // Index lookups by key.
    ("Handler_read_key", false),
    ("Created_tmp_tables", false),
    // On disk, which is the one that hurts.
    ("Created_tmp_disk_tables", true),
    // The sort did not fit in sort_buffer_size.
    ("Sort_merge_passes", true),
    ("Sort_rows", false),
    ("Sort_scan", false),
    // A full scan the optimizer chose.
    ("Select_scan", true),
    // A join with no usable index: the expensive one.
    ("Select_full_join", true),
    ("Select_range", false),
];

/// Subtract two `SHOW SESSION STATUS` reads.
///
/// Counters that did not move are dropped: a panel of zeros reads as a panel
/// of problems, and the ones that matter are then hard to find. `notable` is
/// carried through so the view can mark the few that mean something on their
/// own.
pub fn counter_delta(before: &[(String, i64)], after: &[(String, i64)]) -> Vec<Counter> {
    let get = |list: &[(String, i64)], name: &str| -> Option<i64> {
        list.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    };

    let mut out = Vec::new();
    for (name, notable) in WATCHED {
        let (Some(a), Some(b)) = (get(before, name), get(after, name)) else {
            continue;
        };
        let delta = b - a;
        if delta <= 0 {
            continue;
        }
        out.push(Counter {
            name: (*name).to_string(),
            delta,
            notable: *notable,
        });
    }
    out
}

// --------------------------------------------------------------- verdicts

/// An estimate this far from the truth means the plan was chosen for a
/// different query than the one that ran.
const ESTIMATE_OFF_BY: f64 = 10.0;

/// Below this, a scan is cheaper than the index lookups that would replace it.
/// Picked to be obviously safe rather than tuned: the point is to not call a
/// 400-row lookup table a problem.
const SCAN_ROWS_WORTH_MENTIONING: f64 = 10_000.0;

/// A per-row operation repeated this often is a shape, not an accident.
const LOOPS_WORTH_MENTIONING: f64 = 1_000.0;

/// Read the plan and the counters, and say what happened.
///
/// A pure function of its inputs, so every verdict in the UI can be produced in
/// a test from a plan that is written down rather than measured.
pub fn verdicts(plan: &[PlanNode], counters: &[Counter], rows_sent: Option<f64>) -> Vec<Finding> {
    let mut out = Vec::new();
    let counter = |name: &str| counters.iter().find(|c| c.name == name).map(|c| c.delta);

    // The widest scan in the plan, which is the one worth naming.
    let scan = plan
        .iter()
        .filter(|n| n.op.starts_with("Table scan"))
        .max_by(|a, b| {
            rows_of(a)
                .partial_cmp(&rows_of(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    // **The rule inherited from the dock**: a statement that returns
    // everything it reads cannot be helped by an index, so no index is
    // proposed for one. `acl_rule` — 3,621 scans, 1.51 billion rows, 1:1 — is
    // the counter-example this exists for.
    let returns_what_it_reads = match (scan.and_then(rows_opt), rows_sent) {
        (Some(read), Some(sent)) if read > 0.0 && sent > 0.0 => read / sent < 2.0,
        _ => false,
    };

    if let Some(node) = scan {
        let rows = rows_of(node);
        if rows >= SCAN_ROWS_WORTH_MENTIONING {
            let table = node.op.strip_prefix("Table scan on ").unwrap_or("").trim();
            if returns_what_it_reads {
                out.push(Finding {
                    code: "returns-what-it-reads".into(),
                    title: "A scan, but no index would help".into(),
                    severity: Severity::Note,
                    evidence: format!(
                        "{} was read end to end — {} rows — and nearly all of them were returned. \
                         An index narrows a search; it cannot narrow a query that wants everything.",
                        display_table(table),
                        thousands(rows)
                    ),
                    suggestion: None,
                });
            } else {
                let kept = rows_sent.map(thousands).unwrap_or_else(|| "fewer".into());
                out.push(Finding {
                    code: "full-scan".into(),
                    title: "A scan where a lookup would do".into(),
                    severity: Severity::High,
                    evidence: format!(
                        "{} was read end to end — {} rows — to return {}.",
                        display_table(table),
                        thousands(rows),
                        kept
                    ),
                    suggestion: index_candidate(plan, table),
                });
            }
        }
    }

    // The optimizer planned for a different query than the one that ran.
    if let Some((node, factor)) = plan
        .iter()
        .filter_map(|n| match (n.est_rows, n.actual_rows) {
            (Some(e), Some(a)) if e > 0.0 && a > 0.0 => Some((n, a / e)),
            _ => None,
        })
        .filter(|(_, f)| *f >= ESTIMATE_OFF_BY)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        out.push(Finding {
            code: "estimate-off".into(),
            title: format!("The optimizer was wrong by {}×", factor.round() as i64),
            severity: Severity::High,
            evidence: format!(
                "{} expected {} rows and produced {}. A plan chosen for {} rows is not the plan \
                 for {}, which is how the work below it multiplied.",
                node.op,
                thousands(node.est_rows.unwrap_or(0.0)),
                thousands(node.actual_rows.unwrap_or(0.0)),
                thousands(node.est_rows.unwrap_or(0.0)),
                thousands(node.actual_rows.unwrap_or(0.0))
            ),
            suggestion: None,
        });
    }

    // A sort that has to see everything before it can return the first row.
    if let Some(node) = plan.iter().find(|n| n.op.starts_with("Sort")) {
        if let (Some(input), Some(sent)) = (sort_input(plan, node), rows_sent) {
            if input > sent * 10.0 && input >= SCAN_ROWS_WORTH_MENTIONING {
                out.push(Finding {
                    code: "sort-for-few".into(),
                    title: "The sort reads everything to return a few".into(),
                    severity: Severity::Medium,
                    evidence: format!(
                        "{} rows were sorted to return {}. No index supplies this order, so the \
                         first row cannot be returned until the last one has been read.",
                        thousands(input),
                        thousands(sent)
                    ),
                    suggestion: None,
                });
            }
        }
    }

    // One row at a time, very many times.
    if let Some(node) = plan
        .iter()
        .filter(|n| n.loops.unwrap_or(0.0) >= LOOPS_WORTH_MENTIONING)
        .max_by(|a, b| {
            a.loops
                .unwrap_or(0.0)
                .partial_cmp(&b.loops.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        let loops = node.loops.unwrap_or(0.0);
        let cost = node.self_ms.unwrap_or(0.0);
        out.push(Finding {
            code: "loops".into(),
            title: "A per-row operation, repeated".into(),
            severity: if cost >= 100.0 {
                Severity::High
            } else {
                Severity::Medium
            },
            evidence: format!(
                "{} ran {} times and cost {:.0} ms of the measured time. It is fast each time; \
                 there are simply too many times.",
                node.op,
                thousands(loops),
                cost
            ),
            suggestion: None,
        });
    }

    if counter("Created_tmp_disk_tables").unwrap_or(0) > 0 {
        out.push(Finding {
            code: "tmp-disk".into(),
            title: "A temporary table went to disk".into(),
            severity: Severity::High,
            evidence: "The server could not hold the intermediate result in memory and wrote it \
                       out. This is usually a row count or a row width that grew past \
                       tmp_table_size, and it is paid in I/O."
                .into(),
            suggestion: None,
        });
    }

    if counter("Sort_merge_passes").unwrap_or(0) > 0 {
        out.push(Finding {
            code: "sort-merge".into(),
            title: "The sort did not fit in memory".into(),
            severity: Severity::Medium,
            evidence: format!(
                "{} merge passes: the rows being sorted were larger than sort_buffer_size, so the \
                 sort was done in chunks and merged.",
                counter("Sort_merge_passes").unwrap_or(0)
            ),
            suggestion: None,
        });
    }

    if out.is_empty() {
        out.push(Finding {
            code: "nothing-to-report".into(),
            title: "Nothing here looks wrong".into(),
            severity: Severity::Note,
            evidence: "No scan worth naming, no estimate far from the truth, no temporary table \
                       on disk. If this statement is slow, it is slow for a reason this panel \
                       cannot see from one run."
                .into(),
            suggestion: None,
        });
    }

    out
}

/// Rows a node actually produced, falling back to the estimate.
fn rows_of(n: &PlanNode) -> f64 {
    rows_opt(n).unwrap_or(0.0)
}

fn rows_opt(n: &PlanNode) -> Option<f64> {
    n.actual_rows.or(n.est_rows)
}

/// What went into the sort: its single child.
fn sort_input(plan: &[PlanNode], sort: &PlanNode) -> Option<f64> {
    let idx = plan.iter().position(|n| std::ptr::eq(n, sort))?;
    plan.iter()
        .skip(idx + 1)
        .take_while(|n| n.depth > sort.depth)
        .find(|n| n.depth == sort.depth + 1)
        .and_then(rows_opt)
}

/// A composite index to try, built from the filter under the scan and the sort
/// above it.
///
/// **Conservative on purpose.** The columns come from the optimizer's rendering
/// of the predicate, not from the user's SQL, so anything this cannot read with
/// confidence produces no suggestion at all. A verdict with no suggestion is
/// honest; a suggestion that names a column that does not exist is not.
fn index_candidate(plan: &[PlanNode], table: &str) -> Option<String> {
    if table.is_empty() || table.contains(' ') {
        return None;
    }
    let filter = plan
        .iter()
        .find(|n| n.op.starts_with("Filter") && !n.detail.is_empty())?;

    let mut cols = equality_columns(&filter.detail);
    if cols.is_empty() {
        return None;
    }

    if let Some(order) = plan
        .iter()
        .find(|n| n.op.starts_with("Sort"))
        .and_then(|n| sort_column(&n.detail))
    {
        if !cols.contains(&order) {
            cols.push(order);
        }
    }

    Some(format!(
        "CREATE INDEX ix_{}_{} ON {} ({}) INVISIBLE;",
        table.to_ascii_lowercase(),
        cols.join("_").to_ascii_lowercase(),
        table,
        cols.join(", ")
    ))
}

/// Columns compared for equality with a constant: `(s.tenant_sub_id = 4821)`.
///
/// An `OR` is skipped entirely — one index cannot serve both sides, and
/// proposing one that appears to would be worse than proposing nothing.
fn equality_columns(detail: &str) -> Vec<String> {
    if detail.to_ascii_lowercase().contains(" or ") {
        return Vec::new();
    }
    let mut cols = Vec::new();
    for part in detail.split("and") {
        let Some((lhs, rhs)) = part.split_once('=') else {
            continue;
        };
        if rhs.trim_start().starts_with('=') {
            continue;
        }
        let name = lhs
            .trim()
            .trim_start_matches('(')
            .trim()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches(')')
            .trim();
        if is_plain_ident(name) && !cols.contains(&name.to_string()) {
            cols.push(name.to_string());
        }
    }
    cols
}

/// The first key of `s.updated_at DESC, limit input to 50 row(s) per chunk`.
fn sort_column(detail: &str) -> Option<String> {
    let first = detail.split(',').next()?;
    let name = first
        .split_whitespace()
        .next()?
        .rsplit('.')
        .next()?
        .trim_end_matches(')');
    is_plain_ident(name).then(|| name.to_string())
}

fn is_plain_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn display_table(table: &str) -> String {
    if table.is_empty() {
        "A table".to_string()
    } else {
        table.to_string()
    }
}

/// `1204117` → `1,204,117`. Figures in this panel are read, not computed with.
fn thousands(v: f64) -> String {
    let n = v.round().abs() as u64;
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if v < 0.0 {
        format!("-{out}")
    } else {
        out
    }
}

/// `EXPLAIN FORMAT=TREE` needs 8.0.16, `EXPLAIN ANALYZE` needs 8.0.18.
///
/// Parsed from the server's own version string, which can carry anything after
/// the numbers (`8.0.36-aurora.3.07.1`). An unreadable version is treated as
/// too old: the classic tabular `EXPLAIN` works everywhere, so the fallback is
/// never empty.
pub fn plan_support(version: &str) -> (bool, bool) {
    let nums: Vec<u32> = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(3)
        .filter_map(|s| s.parse().ok())
        .collect();
    let [major, minor, patch] = nums[..] else {
        return (false, false);
    };
    let at_least = |a: u32, b: u32, c: u32| (major, minor, patch) >= (a, b, c);
    (at_least(8, 0, 16), at_least(8, 0, 18))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from the canvas, as the server prints it.
    const MEASURED: &str = "\
-> Limit: 50 row(s)  (cost=1.2 rows=50) (actual time=842..842 rows=50 loops=1)
    -> Sort: s.updated_at DESC, limit input to 50 row(s) per chunk  (actual time=842..842 rows=118944 loops=1)
        -> Stream results  (cost=48913 rows=119204) (actual time=0.09..712 rows=118944 loops=1)
            -> Nested loop inner join  (cost=48913 rows=119204) (actual time=0.08..712 rows=118944 loops=1)
                -> Filter: ((s.tenant_sub_id = 4821) and (s.NAME = 'ACME'))  (cost=7121 rows=11920) (actual time=0.05..268 rows=118944 loops=1)
                    -> Table scan on Supplier  (cost=7121 rows=119204) (actual time=0.04..196 rows=1204117 loops=1)
                -> Single-row index lookup on p using PRIMARY (party_id=s.party_id)  (cost=0.25 rows=1) (actual time=0.003..0.003 rows=1 loops=118944)
";

    // ------------------------------------------------------------ the gate

    /// **The one that must not regress.** `EXPLAIN ANALYZE` executes what it is
    /// given, and the manual lists multi-table UPDATE and DELETE among the
    /// statements it accepts. Every one of these must be refused from the text
    /// alone, before a connection exists.
    #[test]
    fn only_a_row_returning_statement_may_be_diagnosed() {
        assert!(may_diagnose("SELECT 1").is_ok());
        assert!(may_diagnose("  select a from t where b = 1").is_ok());
        assert!(may_diagnose("TABLE Supplier").is_ok());

        for sql in [
            "DELETE a FROM a JOIN b ON a.id = b.id",
            "UPDATE a JOIN b ON a.id = b.id SET a.x = 1",
            "DELETE FROM Supplier WHERE id = 1",
            "DROP TABLE Supplier",
            "INSERT INTO t VALUES (1)",
        ] {
            assert_eq!(
                may_diagnose(sql),
                Err(Refusal::Writes),
                "must refuse: {sql}"
            );
        }
    }

    #[test]
    fn an_empty_buffer_is_refused_as_empty_not_as_a_write() {
        assert_eq!(may_diagnose("   \n-- just a note\n"), Err(Refusal::Empty));
    }

    // ---------------------------------------------------------- the parser

    #[test]
    fn the_tree_is_read_into_nodes_with_their_depths() {
        let plan = parse_tree(MEASURED);
        assert_eq!(plan.len(), 7);
        assert_eq!(plan[0].op, "Limit");
        assert_eq!(plan[0].depth, 0);
        assert_eq!(plan[5].op, "Table scan on Supplier");
        assert_eq!(plan[5].depth, 5);
        // The key expression is part of the label, not a figures group: it is
        // what the lookup is *on*, and dropping it would lose which column
        // drives the join.
        assert_eq!(
            plan[6].op,
            "Single-row index lookup on p using PRIMARY (party_id=s.party_id)"
        );
        assert_eq!(plan[6].depth, 4);
    }

    /// A predicate has parentheses of its own, and they belong to the label.
    #[test]
    fn a_predicate_keeps_its_own_parentheses() {
        let plan = parse_tree(MEASURED);
        let filter = &plan[4];
        assert_eq!(filter.op, "Filter");
        assert_eq!(
            filter.detail,
            "((s.tenant_sub_id = 4821) and (s.NAME = 'ACME'))"
        );
        assert_eq!(filter.est_rows, Some(11920.0));
        assert_eq!(filter.actual_rows, Some(118944.0));
    }

    #[test]
    fn figures_are_read_from_both_groups() {
        let plan = parse_tree(MEASURED);
        let scan = &plan[5];
        assert_eq!(scan.cost, Some(7121.0));
        assert_eq!(scan.est_rows, Some(119204.0));
        assert_eq!(scan.actual_rows, Some(1204117.0));
        assert_eq!(scan.loops, Some(1.0));
        assert_eq!(scan.time_ms, Some(196.0));
    }

    /// **The subtlety this module exists to get right.** The lookup is printed
    /// at 0.003 ms, which looks like nothing — until it is multiplied by its
    /// 118,944 loops, where it becomes the largest single cost in the
    /// statement.
    #[test]
    fn self_time_multiplies_loops_and_subtracts_children() {
        let plan = parse_tree(MEASURED);

        let lookup = &plan[6];
        assert_eq!(lookup.total_ms, Some(0.003 * 118_944.0));
        assert!((lookup.self_ms.unwrap() - 356.8).abs() < 0.5);

        // The scan has no children, so its total is its own.
        assert_eq!(plan[5].self_ms, Some(196.0));

        // The filter's 268 ms includes the scan's 196.
        assert_eq!(plan[4].self_ms, Some(72.0));

        // The root is 842 ms cumulative; the sort under it is also 842, so the
        // limit itself cost nothing.
        assert_eq!(plan[0].self_ms, Some(0.0));
    }

    /// A node whose children are printed as costing slightly more than it does
    /// lands at zero, not below it.
    #[test]
    fn rounding_in_the_servers_output_cannot_make_self_time_negative() {
        let plan = parse_tree(
            "-> Outer  (actual time=1..10 rows=1 loops=1)\n    -> Inner  (actual time=1..11 rows=1 loops=1)\n",
        );
        assert_eq!(plan[0].self_ms, Some(0.0));
    }

    /// The free plan has no `actual` group at all, and must still parse.
    #[test]
    fn an_estimate_only_plan_parses_without_timings() {
        let plan = parse_tree("-> Table scan on Supplier  (cost=7121 rows=119204)\n");
        assert_eq!(plan[0].est_rows, Some(119204.0));
        assert_eq!(plan[0].actual_rows, None);
        assert_eq!(plan[0].self_ms, None);
    }

    #[test]
    fn a_branch_that_never_ran_says_so() {
        let plan = parse_tree("-> Index lookup on t  (never executed)\n");
        assert!(plan[0].never_executed);
    }

    /// **The parser must not be the thing that breaks.** This output is
    /// documentation, not an interface.
    #[test]
    fn a_line_it_cannot_read_becomes_a_node_rather_than_an_error() {
        let plan = parse_tree("-> : something new in a later version\n");
        assert_eq!(plan.len(), 1);
        assert!(plan[0].unparsed);
        assert!(plan[0].op.contains("something new"));
    }

    // ------------------------------------------------------- normalisation

    /// **The decision that keeps data off the network.** Every value the person
    /// typed becomes a placeholder; the shape the assistant needs survives.
    #[test]
    fn literals_are_replaced_and_structure_is_kept() {
        let got = normalise(
            "SELECT a FROM Supplier WHERE tenant_sub_id = 4821 AND NAME = 'Northwind Trading'",
        );
        assert_eq!(
            got,
            "SELECT a FROM Supplier WHERE tenant_sub_id = ? AND NAME = ?"
        );
    }

    #[test]
    fn a_comment_is_dropped_because_it_can_hold_anything() {
        let got = normalise("SELECT a -- ticket OPS-4471, ask Dana\nFROM t");
        assert!(!got.contains("Dana"), "{got}");
        assert!(!got.contains("4471"), "{got}");
        assert!(got.contains("SELECT a"), "{got}");
        assert!(got.contains("FROM t"), "{got}");
    }

    /// A quoted identifier is a name, not a value.
    #[test]
    fn a_backticked_name_survives_but_a_string_does_not() {
        let got = normalise("SELECT `order` FROM `Supplier` WHERE x = 'order'");
        assert!(got.contains("`order`"), "{got}");
        assert!(got.contains("`Supplier`"), "{got}");
        assert!(!got.contains("'order'"), "{got}");
    }

    /// `t1` is a name. The digit in it is not a literal, and replacing it would
    /// produce SQL that no longer parses.
    #[test]
    fn a_digit_inside_an_identifier_is_not_a_literal() {
        assert_eq!(
            normalise("SELECT x FROM t1 WHERE a = 1"),
            "SELECT x FROM t1 WHERE a = ?"
        );
    }

    /// The whole point of masking: a keyword inside a string is not code, and
    /// a quote inside a comment does not open a literal.
    #[test]
    fn a_quote_in_a_comment_cannot_open_a_literal() {
        let got = normalise("SELECT a FROM t /* don't */ WHERE b = 'x' AND c = 2");
        assert_eq!(got, "SELECT a FROM t WHERE b = ? AND c = ?");
    }

    // ------------------------------------------------------------ counters

    #[test]
    fn only_counters_that_moved_are_reported() {
        let before = vec![
            ("Handler_read_rnd_next".to_string(), 10),
            ("Sort_merge_passes".to_string(), 3),
            ("Select_scan".to_string(), 1),
        ];
        let after = vec![
            ("Handler_read_rnd_next".to_string(), 1_204_128),
            ("Sort_merge_passes".to_string(), 3),
            ("Select_scan".to_string(), 2),
        ];
        let got = counter_delta(&before, &after);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].name, "Handler_read_rnd_next");
        assert_eq!(got[0].delta, 1_204_118);
        assert!(got[0].notable);
        assert_eq!(got[1].name, "Select_scan");
    }

    #[test]
    fn a_counter_the_server_does_not_have_is_skipped_not_reported_as_zero() {
        let got = counter_delta(&[], &[]);
        assert!(got.is_empty());
    }

    // ------------------------------------------------------------ verdicts

    #[test]
    fn the_scan_the_estimate_and_the_loops_are_all_found() {
        let plan = parse_tree(MEASURED);
        let found = verdicts(&plan, &[], Some(50.0));
        let codes: Vec<&str> = found.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"full-scan"), "{codes:?}");
        assert!(codes.contains(&"estimate-off"), "{codes:?}");
        assert!(codes.contains(&"sort-for-few"), "{codes:?}");
        assert!(codes.contains(&"loops"), "{codes:?}");
    }

    /// **The dock's rule, inherited.** `acl_rule` reads 1.51 billion rows and
    /// returns them; no index can help a query that wants everything. The
    /// verdict must say so *and* propose nothing.
    #[test]
    fn no_index_is_proposed_for_a_statement_that_returns_what_it_reads() {
        let plan = parse_tree(
            "-> Table scan on acl_rule  (cost=1 rows=1510000) (actual time=0.1..900 rows=1510000 loops=1)\n",
        );
        let found = verdicts(&plan, &[], Some(1_510_000.0));
        let scan = found
            .iter()
            .find(|f| f.code == "returns-what-it-reads")
            .expect("the 1:1 case must be named");
        assert!(scan.suggestion.is_none(), "{scan:?}");
        assert!(!found.iter().any(|f| f.code == "full-scan"));
    }

    #[test]
    fn a_small_table_scan_is_not_a_finding() {
        let plan = parse_tree(
            "-> Table scan on currency  (cost=1 rows=180) (actual time=0.1..0.3 rows=180 loops=1)\n",
        );
        let found = verdicts(&plan, &[], Some(180.0));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "nothing-to-report");
    }

    /// The suggestion is composite: the equality column narrows, the sort
    /// column removes the sort. One without the other leaves half the cost.
    #[test]
    fn the_index_candidate_carries_the_filter_and_the_sort_column() {
        let plan = parse_tree(
            "-> Sort: s.updated_at DESC  (actual time=8..8 rows=50 loops=1)\n    -> Filter: (s.tenant_sub_id = 4821)  (cost=7121 rows=11920) (actual time=0.05..268 rows=118944 loops=1)\n        -> Table scan on Supplier  (cost=7121 rows=119204) (actual time=0.04..196 rows=1204117 loops=1)\n",
        );
        let found = verdicts(&plan, &[], Some(50.0));
        let scan = found.iter().find(|f| f.code == "full-scan").unwrap();
        assert_eq!(
            scan.suggestion.as_deref(),
            Some("CREATE INDEX ix_supplier_tenant_sub_id_updated_at ON Supplier (tenant_sub_id, updated_at) INVISIBLE;")
        );
    }

    /// **An OR across two columns cannot be served by one index.** Proposing
    /// one that looks as though it could is worse than proposing nothing.
    #[test]
    fn an_or_predicate_produces_no_index_suggestion() {
        let plan = parse_tree(
            "-> Filter: ((s.tenant_sub_id = 4821) and ((s.NAME = 'A') or (s.alt_name = 'A')))  (cost=1 rows=100) (actual time=1..2 rows=118944 loops=1)\n    -> Table scan on Supplier  (cost=1 rows=119204) (actual time=1..2 rows=1204117 loops=1)\n",
        );
        let found = verdicts(&plan, &[], Some(50.0));
        let scan = found.iter().find(|f| f.code == "full-scan").unwrap();
        assert!(scan.suggestion.is_none(), "{scan:?}");
    }

    #[test]
    fn a_temporary_table_on_disk_is_its_own_finding() {
        let counters = vec![Counter {
            name: "Created_tmp_disk_tables".into(),
            delta: 1,
            notable: true,
        }];
        let found = verdicts(&[], &counters, None);
        assert!(found.iter().any(|f| f.code == "tmp-disk"));
    }

    #[test]
    fn a_clean_statement_says_so_rather_than_showing_an_empty_panel() {
        let found = verdicts(&[], &[], Some(10.0));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "nothing-to-report");
        assert_eq!(found[0].severity, Severity::Note);
    }

    // ------------------------------------------------------------- version

    #[test]
    fn aurora_and_community_version_strings_both_parse() {
        assert_eq!(plan_support("8.0.36-aurora.3.07.1"), (true, true));
        assert_eq!(plan_support("8.4.0"), (true, true));
        assert_eq!(plan_support("8.0.17"), (true, false));
        assert_eq!(plan_support("8.0.15"), (false, false));
        assert_eq!(plan_support("5.7.44-log"), (false, false));
    }

    /// An unreadable version is treated as the oldest, not the newest: the
    /// classic `EXPLAIN` works everywhere, so the fallback is never empty.
    #[test]
    fn an_unreadable_version_falls_back_rather_than_assuming() {
        assert_eq!(plan_support(""), (false, false));
        assert_eq!(plan_support("unknown"), (false, false));
    }
}
