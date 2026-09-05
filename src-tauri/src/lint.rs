//! Advisory SQL linting.
//!
//! **The linter never blocks execution.** It has no veto: no disabled run
//! button, no confirmation dialog. It is a second opinion and it will sometimes
//! be wrong, so every check here is biased toward staying quiet. A linter that
//! cries wolf gets ignored within a week, and then the one time it is right
//! nobody looks.
//!
//! It lives in Rust rather than the frontend for two reasons: it reuses the
//! execution scanner (so "inside a string" means exactly what it means to the
//! splitter), and the schema cache it needs is already here.

use std::collections::HashMap;

use serde::Serialize;

use crate::exec::{self, StatementKind};
use crate::split;

/// Lowercased table name -> lowercased column names.
pub type LintSchema = HashMap<String, Vec<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Byte offsets into the whole buffer, matching the splitter's convention.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    pub message: String,
}

// ---------------------------------------------------------------- tokenizer

#[derive(Debug, Clone)]
struct Tok {
    start: usize,
    end: usize,
    text: String,
    upper: String,
    depth: i32,
    /// Starts with a letter/underscore — a candidate identifier or keyword.
    is_word: bool,
}

/// Tokenize already-masked text. `base` is the statement's offset in the
/// buffer, so every span we emit is absolute.
fn tokenize(masked: &str, base: usize) -> Vec<Tok> {
    let b = masked.as_bytes();
    let mut toks = Vec::new();
    let mut depth = 0i32;
    let mut i = 0usize;

    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;

        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            // Read a possibly dotted identifier: `u.email`, `db.tbl.col`.
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] == b'.')
            {
                i += 1;
            }
            // A trailing dot belongs to the next token (e.g. `t.*`).
            if b[i - 1] == b'.' {
                i -= 1;
            }
            let text = masked[start..i].to_string();
            toks.push(Tok {
                start: base + start,
                end: base + i,
                upper: text.to_ascii_uppercase(),
                text,
                depth,
                is_word: true,
            });
            continue;
        }

        if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            let text = masked[start..i].to_string();
            toks.push(Tok {
                start: base + start,
                end: base + i,
                upper: text.to_ascii_uppercase(),
                text,
                depth,
                is_word: false,
            });
            continue;
        }

        // Single-character punctuation.
        i += 1;
        let text = masked[start..i].to_string();
        let tok_depth = depth;
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
        }
        toks.push(Tok {
            start: base + start,
            end: base + i,
            upper: text.clone(),
            text,
            depth: if c == b')' { depth } else { tok_depth },
            is_word: false,
        });
    }
    toks
}

/// Sorted for binary search. Deliberately broad: a missing keyword shows up as
/// a false "unknown column", which is exactly the noise we must not produce.
const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "AVG",
    "BETWEEN",
    "BIGINT",
    "BINARY",
    "BOOLEAN",
    "BOTH",
    "BY",
    "CASE",
    "CAST",
    "CHAR",
    "CHARACTER",
    "CHECK",
    "COALESCE",
    "COLLATE",
    "COLUMN",
    "CONSTRAINT",
    "CONVERT",
    "COUNT",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DATABASE",
    "DATE",
    "DATETIME",
    "DAY",
    "DECIMAL",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DIV",
    "DOUBLE",
    "DROP",
    "DUPLICATE",
    "ELSE",
    "END",
    "ENUM",
    "ESCAPE",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FLOAT",
    "FOR",
    "FORCE",
    "FOREIGN",
    "FROM",
    "FULL",
    "GROUP",
    "HAVING",
    "HOUR",
    "IF",
    "IGNORE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INT",
    "INTEGER",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LEADING",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOCK",
    "LONGBLOB",
    "LONGTEXT",
    "MAX",
    "MEDIUMINT",
    "MIN",
    "MINUTE",
    "MOD",
    "MONTH",
    "NATURAL",
    "NOT",
    "NOW",
    "NULL",
    "NULLIF",
    "NUMERIC",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PARTITION",
    "PRIMARY",
    "PROCEDURE",
    "REFERENCES",
    "REGEXP",
    "RENAME",
    "REPLACE",
    "RIGHT",
    "RLIKE",
    "SECOND",
    "SELECT",
    "SEPARATOR",
    "SET",
    "SHOW",
    "SIGNED",
    "SMALLINT",
    "SOME",
    "STRAIGHT_JOIN",
    "SUM",
    "TABLE",
    "TEXT",
    "THEN",
    "TIME",
    "TIMESTAMP",
    "TINYINT",
    "TRAILING",
    "TRUE",
    "TRUNCATE",
    "UNION",
    "UNIQUE",
    "UNSIGNED",
    "UPDATE",
    "USE",
    "USING",
    "VALUES",
    "VARBINARY",
    "VARCHAR",
    "WHEN",
    "WHERE",
    "WITH",
    "XOR",
    "YEAR",
];

fn is_keyword(upper: &str) -> bool {
    KEYWORDS.binary_search(&upper).is_ok()
}

// ------------------------------------------------------------------- checks

pub fn lint(sql: &str, schema: &LintSchema) -> Vec<Diagnostic> {
    let out = split::split(sql);

    // A DELIMITER block redefines the terminator, so our statement boundaries
    // are meaningless inside it. Suppress everything rather than guess.
    if out.delimiter_detected {
        return Vec::new();
    }

    // A lexical break makes every downstream check unreliable, so report just
    // that one thing rather than an avalanche of consequences.
    if let Some(u) = split::unterminated(sql) {
        return vec![Diagnostic {
            start: u.start,
            end: (u.start + 1).min(sql.len()),
            severity: Severity::Error,
            message: format!("Unterminated {} — it is never closed.", u.kind),
        }];
    }

    let mut diags = Vec::new();
    for span in &out.statements {
        let text = &sql[span.start..span.end];
        lint_statement(text, span.start, schema, &mut diags);
    }
    diags
}

fn lint_statement(text: &str, base: usize, schema: &LintSchema, diags: &mut Vec<Diagnostic>) {
    let masked = split::mask_keep_idents(text);
    let toks = tokenize(&masked, base);
    if toks.is_empty() {
        return;
    }
    let kind = exec::classify(text);

    check_parens(&toks, diags);
    check_missing_where(&toks, kind, diags);
    check_select_star(&toks, kind, diags);

    // Schema checks stay silent until the cache for this database has loaded —
    // otherwise every table looks unknown on a cold start.
    if !schema.is_empty() {
        check_schema(&toks, schema, diags);
    }
}

fn check_parens(toks: &[Tok], diags: &mut Vec<Diagnostic>) {
    let mut stack: Vec<&Tok> = Vec::new();
    for t in toks {
        if t.text == "(" {
            stack.push(t);
        } else if t.text == ")" && stack.pop().is_none() {
            diags.push(Diagnostic {
                start: t.start,
                end: t.end,
                severity: Severity::Error,
                message: "Unmatched closing parenthesis.".into(),
            });
        }
    }
    if let Some(open) = stack.first() {
        diags.push(Diagnostic {
            start: open.start,
            end: open.end,
            severity: Severity::Error,
            message: format!(
                "Unclosed parenthesis — {} still open at the end of the statement.",
                stack.len()
            ),
        });
    }
}

fn check_missing_where(toks: &[Tok], kind: StatementKind, diags: &mut Vec<Diagnostic>) {
    if kind != StatementKind::Modify {
        return;
    }
    let verb = &toks[0].upper;
    if verb != "DELETE" && verb != "UPDATE" {
        return; // INSERT / REPLACE have no WHERE to miss.
    }
    if toks.iter().any(|t| t.depth == 0 && t.upper == "WHERE") {
        return;
    }
    diags.push(Diagnostic {
        start: toks[0].start,
        end: toks[0].end,
        severity: Severity::Warning,
        message: format!("{verb} without a WHERE clause affects every row in the table."),
    });
}

fn check_select_star(toks: &[Tok], kind: StatementKind, diags: &mut Vec<Diagnostic>) {
    if kind != StatementKind::Select {
        return;
    }
    // Only the top-level projection: a `*` inside COUNT(*) sits at depth 1, and
    // one in a subquery is not what the user is about to page through.
    let from = toks
        .iter()
        .position(|t| t.depth == 0 && t.upper == "FROM")
        .unwrap_or(toks.len());
    let star = toks[..from].iter().find(|t| t.depth == 0 && t.text == "*");
    let Some(star) = star else { return };

    if toks.iter().any(|t| t.depth == 0 && t.upper == "LIMIT") {
        return;
    }
    diags.push(Diagnostic {
        start: star.start,
        end: star.end,
        severity: Severity::Info,
        message: "SELECT * with no LIMIT — every column of every row.".into(),
    });
}

/// A table reference and the alias it was given, if any.
struct TableRef {
    name: String,
    alias: Option<String>,
    start: usize,
    end: usize,
    /// Qualified with an explicit database we cannot introspect.
    foreign: bool,
}

fn collect_tables(toks: &[Tok]) -> Vec<TableRef> {
    let mut refs = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if !(t.upper == "FROM" || t.upper == "JOIN") {
            continue;
        }
        let Some(name_tok) = toks.get(i + 1) else {
            continue;
        };
        // `FROM (SELECT ...)` is a derived table, not a name we can check.
        if !name_tok.is_word || is_keyword(&name_tok.upper) {
            continue;
        }

        // `db.table` — only the unqualified form maps onto our cache.
        let dots = name_tok.text.matches('.').count();
        let bare = name_tok.text.rsplit('.').next().unwrap_or(&name_tok.text);

        let mut alias = None;
        if let Some(next) = toks.get(i + 2) {
            if next.upper == "AS" {
                if let Some(a) = toks.get(i + 3) {
                    if a.is_word {
                        alias = Some(a.text.to_ascii_lowercase());
                    }
                }
            } else if next.is_word && !is_keyword(&next.upper) {
                alias = Some(next.text.to_ascii_lowercase());
            }
        }

        refs.push(TableRef {
            name: bare.to_ascii_lowercase(),
            alias,
            start: name_tok.start,
            end: name_tok.end,
            foreign: dots > 0,
        });
    }
    refs
}

fn check_schema(toks: &[Tok], schema: &LintSchema, diags: &mut Vec<Diagnostic>) {
    let tables = collect_tables(toks);

    // --- unknown tables
    for t in &tables {
        if t.foreign || schema.contains_key(&t.name) {
            continue;
        }
        diags.push(Diagnostic {
            start: t.start,
            end: t.end,
            severity: Severity::Warning,
            message: format!("Unknown table `{}` in the active database.", t.name),
        });
    }

    // alias (or bare table name) -> table
    let mut scope: HashMap<String, String> = HashMap::new();
    for t in &tables {
        if t.foreign {
            continue;
        }
        if let Some(a) = &t.alias {
            scope.insert(a.clone(), t.name.clone());
        }
        scope.insert(t.name.clone(), t.name.clone());
    }

    let has_column = |table: &str, col: &str| -> bool {
        schema
            .get(table)
            .is_some_and(|cols| cols.iter().any(|c| c == col))
    };

    // --- qualified columns: `u.email`. Reliable, because the alias tells us
    //     exactly which table to look in.
    for t in toks.iter().filter(|t| t.is_word && t.text.contains('.')) {
        let (left, col) = t.text.rsplit_once('.').unwrap();
        let left = left.rsplit('.').next().unwrap_or(left).to_ascii_lowercase();
        let col_l = col.to_ascii_lowercase();
        if col == "*" || col.is_empty() {
            continue;
        }
        let Some(table) = scope.get(&left) else {
            continue;
        };
        if !schema.contains_key(table) || has_column(table, &col_l) {
            continue;
        }
        diags.push(Diagnostic {
            start: t.start,
            end: t.end,
            severity: Severity::Warning,
            message: format!("`{table}` has no column `{col}`."),
        });
    }

    // --- unqualified columns. Only when exactly one table is in scope and the
    //     statement has no subquery: with two tables an unqualified name is
    //     genuinely ambiguous, and we would rather say nothing than guess.
    if tables.len() != 1 || tables[0].foreign {
        return;
    }
    if toks.iter().any(|t| t.depth > 0 && t.upper == "SELECT") {
        return;
    }
    let table = &tables[0].name;
    if !schema.contains_key(table) {
        return;
    }

    // Names introduced by `AS` are the user's own; they are not columns.
    let mut declared: Vec<String> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.upper == "AS" {
            if let Some(a) = toks.get(i + 1) {
                if a.is_word {
                    declared.push(a.text.to_ascii_lowercase());
                }
            }
        }
    }

    for (i, t) in toks.iter().enumerate() {
        if !t.is_word || t.text.contains('.') || is_keyword(&t.upper) {
            continue;
        }
        // A name followed by `(` is a function call, not a column.
        if toks.get(i + 1).is_some_and(|n| n.text == "(") {
            continue;
        }
        // Skip the table itself, its alias, and anything the query declared.
        let lower = t.text.to_ascii_lowercase();
        if scope.contains_key(&lower) || declared.contains(&lower) {
            continue;
        }
        // Skip the name immediately after AS (it is being defined, not read).
        if i > 0 && toks[i - 1].upper == "AS" {
            continue;
        }
        if has_column(table, &lower) {
            continue;
        }
        diags.push(Diagnostic {
            start: t.start,
            end: t.end,
            severity: Severity::Warning,
            message: format!("`{table}` has no column `{}`.", t.text),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> LintSchema {
        let mut m = LintSchema::new();
        m.insert(
            "users".into(),
            vec![
                "id".into(),
                "email".into(),
                "display_name".into(),
                "balance".into(),
                "created_at".into(),
            ],
        );
        m.insert(
            "orders".into(),
            vec!["id".into(), "user_id".into(), "total".into()],
        );
        m
    }

    fn msgs(sql: &str) -> Vec<String> {
        lint(sql, &schema())
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    fn none(sql: &str) {
        let d = lint(sql, &schema());
        assert!(d.is_empty(), "expected silence for {sql:?}, got {d:?}");
    }

    #[test]
    fn keyword_table_is_sorted_for_binary_search() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS, "KEYWORDS must stay sorted");
    }

    // ---- Tier 1

    #[test]
    fn flags_delete_and_update_without_where() {
        assert!(msgs("DELETE FROM users")[0].contains("DELETE without a WHERE"));
        assert!(msgs("UPDATE users SET balance = 0")[0].contains("UPDATE without a WHERE"));
    }

    #[test]
    fn accepts_delete_and_update_with_where() {
        none("DELETE FROM users WHERE id = 1");
        none("UPDATE users SET balance = 0 WHERE id = 1");
    }

    #[test]
    fn a_where_inside_a_subquery_does_not_count_as_the_outer_where() {
        let m = msgs("DELETE FROM users WHERE id IN (SELECT user_id FROM orders WHERE total > 1)");
        assert!(m.is_empty(), "{m:?}");
        let m = msgs("DELETE FROM users");
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn insert_is_not_flagged_for_a_missing_where() {
        none("INSERT INTO users (email) VALUES ('a@b.c')");
    }

    #[test]
    fn flags_unterminated_quote_as_an_error_and_nothing_else() {
        let d = lint("SELECT * FROM users WHERE email = 'oops", &schema());
        assert_eq!(d.len(), 1, "should not cascade: {d:?}");
        assert_eq!(d[0].severity, Severity::Error);
        assert!(d[0].message.contains("Unterminated string literal"));
    }

    #[test]
    fn flags_unbalanced_parens() {
        let d = lint("SELECT (1 + 2 FROM users", &schema());
        assert!(
            d.iter().any(|x| x.message.contains("Unclosed parenthesis")),
            "{d:?}"
        );
        let d = lint("SELECT 1) FROM users", &schema());
        assert!(
            d.iter().any(|x| x.message.contains("Unmatched closing")),
            "{d:?}"
        );
    }

    #[test]
    fn select_star_without_limit_is_info_only() {
        let d = lint("SELECT * FROM users", &schema());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Info);
    }

    #[test]
    fn select_star_with_limit_is_silent() {
        none("SELECT * FROM users LIMIT 10");
    }

    #[test]
    fn count_star_is_not_select_star() {
        // The `*` sits at depth 1, inside COUNT(...).
        none("SELECT COUNT(*) FROM users");
    }

    // ---- Tier 2

    #[test]
    fn flags_an_unknown_table() {
        assert!(msgs("SELECT id FROM custmers")[0].contains("Unknown table `custmers`"));
    }

    #[test]
    fn resolves_aliases_for_qualified_columns() {
        none("SELECT u.email FROM users u");
        none("SELECT u.email FROM users AS u");
        assert!(msgs("SELECT u.emial FROM users u")[0].contains("no column `emial`"));
    }

    #[test]
    fn checks_qualified_columns_across_a_join() {
        none("SELECT u.email, o.total FROM users u JOIN orders o ON o.user_id = u.id");
        let m = msgs("SELECT u.email, o.totl FROM users u JOIN orders o ON o.user_id = u.id");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("no column `totl`"));
    }

    #[test]
    fn flags_an_unqualified_typo_when_one_table_is_in_scope() {
        assert!(msgs("SELECT emial FROM users")[0].contains("no column `emial`"));
    }

    #[test]
    fn stays_quiet_about_unqualified_columns_when_two_tables_are_in_scope() {
        // Genuinely ambiguous — saying nothing beats guessing wrong.
        none("SELECT email, total FROM users u JOIN orders o ON o.user_id = u.id");
        none("SELECT whatever FROM users u JOIN orders o ON o.user_id = u.id");
    }

    #[test]
    fn stays_quiet_when_the_schema_cache_is_empty() {
        let empty = LintSchema::new();
        assert!(lint("SELECT emial FROM custmers", &empty).is_empty());
    }

    #[test]
    fn suppresses_everything_inside_a_delimiter_block() {
        let sql = "DELIMITER $$\nCREATE PROCEDURE p() BEGIN DELETE FROM users; END $$";
        assert!(lint(sql, &schema()).is_empty());
    }

    // ---- false-positive guards: these must all stay silent

    #[test]
    fn function_calls_are_not_mistaken_for_columns() {
        none("SELECT COUNT(id), MAX(balance), NOW() FROM users WHERE id = 1");
        none("SELECT DATE(created_at) FROM users u WHERE u.id = 1");
    }

    #[test]
    fn select_list_aliases_are_not_mistaken_for_columns() {
        none("SELECT balance AS spend FROM users WHERE id = 1");
        none("SELECT SUM(balance) AS grand_total FROM users WHERE id = 1");
    }

    #[test]
    fn keywords_and_literals_are_not_mistaken_for_columns() {
        none("SELECT id FROM users WHERE email IS NOT NULL ORDER BY id DESC LIMIT 5");
        none("SELECT id FROM users WHERE balance BETWEEN 1 AND 2");
        none("SELECT id FROM users WHERE email LIKE 'a%' AND id IN (1, 2, 3)");
        none("SELECT CASE WHEN balance > 0 THEN 1 ELSE 0 END FROM users WHERE id = 1");
    }

    #[test]
    fn backticked_identifiers_are_still_seen() {
        none("SELECT `email` FROM `users` WHERE `id` = 1");
        assert!(msgs("SELECT `emial` FROM `users`")[0].contains("no column `emial`"));
    }

    #[test]
    fn words_inside_strings_and_comments_are_never_linted() {
        none("SELECT id FROM users WHERE email = 'emial not a column' AND id = 1");
        none("SELECT id FROM users -- emial\nWHERE id = 1");
        none("SELECT id /* emial */ FROM users WHERE id = 1");
    }

    #[test]
    fn a_database_qualified_table_is_left_alone() {
        // We only cache the active database, so we cannot judge otherdb.things.
        none("SELECT x.id FROM otherdb.things x WHERE x.id = 1");
    }

    #[test]
    fn diagnostic_offsets_land_on_the_right_text() {
        let sql = "SELECT emial FROM users";
        let d = lint(sql, &schema());
        assert_eq!(&sql[d[0].start..d[0].end], "emial");
    }

    #[test]
    fn offsets_are_absolute_across_multiple_statements() {
        let sql = "SELECT 1;\nSELECT emial FROM users";
        let d = lint(sql, &schema());
        assert_eq!(&sql[d[0].start..d[0].end], "emial");
    }

    #[test]
    fn multibyte_text_does_not_shift_offsets() {
        let sql = "SELECT 'héllo → x', emial FROM users";
        let d = lint(sql, &schema());
        assert_eq!(&sql[d[0].start..d[0].end], "emial");
    }
}
