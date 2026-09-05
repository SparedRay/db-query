//! MySQL statement splitter.
//!
//! This is the single source of truth for statement boundaries: execution,
//! auto-LIMIT and "statement under cursor" all go through it, so they can
//! never disagree about where one statement ends and the next begins.
//!
//! It is a character scanner, not a parser. It knows just enough MySQL
//! lexical structure to tell a real `;` from one inside a string, an
//! identifier or a comment.

use serde::Serialize;

/// Byte offsets into the original buffer. `start..end` is the statement text
/// with surrounding whitespace and the terminating `;` already stripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitOutput {
    pub statements: Vec<StatementSpan>,
    /// A `DELIMITER` directive was seen. The buffer is returned as one span and
    /// the caller must not rewrite it — auto-LIMIT and per-statement tabs are
    /// meaningless once the terminator is user-defined.
    pub delimiter_detected: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Normal,
    Single,
    Double,
    Backtick,
    LineComment,
    BlockComment,
}

/// True when `--` at `i` actually opens a comment. MySQL requires the double
/// dash to be followed by whitespace or end-of-input; `a--b` is arithmetic,
/// not a comment.
fn is_dash_comment(b: &[u8], i: usize) -> bool {
    if b[i] != b'-' || i + 1 >= b.len() || b[i + 1] != b'-' {
        return false;
    }
    match b.get(i + 2) {
        None => true,
        Some(c) => c.is_ascii_whitespace(),
    }
}

/// Does this buffer contain a `DELIMITER` directive? Only meaningful in
/// Normal mode at the start of a line, which is the only place MySQL clients
/// accept it.
fn has_delimiter_directive(b: &[u8]) -> bool {
    let mut mode = Mode::Normal;
    let mut i = 0;
    let mut at_line_start = true;
    while i < b.len() {
        let c = b[i];
        match mode {
            Mode::Normal => {
                if at_line_start && !c.is_ascii_whitespace() {
                    let rest = &b[i..];
                    if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case(b"DELIMITER") {
                        let after = rest.get(9);
                        if after.is_none() || after.is_some_and(|c| c.is_ascii_whitespace()) {
                            return true;
                        }
                    }
                }
                if !c.is_ascii_whitespace() {
                    at_line_start = false;
                }
                match c {
                    b'\n' => at_line_start = true,
                    b'\'' => mode = Mode::Single,
                    b'"' => mode = Mode::Double,
                    b'`' => mode = Mode::Backtick,
                    b'#' => mode = Mode::LineComment,
                    b'-' if is_dash_comment(b, i) => {
                        mode = Mode::LineComment;
                        i += 1;
                    }
                    b'/' if b.get(i + 1) == Some(&b'*') => {
                        mode = Mode::BlockComment;
                        i += 1;
                    }
                    _ => {}
                }
            }
            Mode::Single | Mode::Double | Mode::Backtick => {
                let q = match mode {
                    Mode::Single => b'\'',
                    Mode::Double => b'"',
                    _ => b'`',
                };
                // Backslash is not an escape character inside backtick-quoted
                // identifiers; only doubling the backtick escapes it.
                if c == b'\\' && mode != Mode::Backtick {
                    i += 1;
                } else if c == q {
                    if b.get(i + 1) == Some(&q) {
                        i += 1;
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::LineComment => {
                if c == b'\n' {
                    mode = Mode::Normal;
                    at_line_start = true;
                }
            }
            Mode::BlockComment => {
                // MySQL block comments do not nest: the first `*/` closes.
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    mode = Mode::Normal;
                    i += 1;
                }
            }
        }
        i += 1;
    }
    false
}

/// Trim whitespace from a span, then report whether anything executable
/// remains. A span holding only comments is not a statement.
fn trim_span(b: &[u8], mut start: usize, mut end: usize) -> Option<StatementSpan> {
    while start < end && b[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if start >= end {
        return None;
    }
    if !has_code(&b[start..end]) {
        return None;
    }
    Some(StatementSpan { start, end })
}

/// Any non-whitespace byte outside a comment?
fn has_code(b: &[u8]) -> bool {
    let mut mode = Mode::Normal;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match mode {
            Mode::Normal => match c {
                b'\'' => return true,
                b'"' => return true,
                b'`' => return true,
                b'#' => mode = Mode::LineComment,
                b'-' if is_dash_comment(b, i) => {
                    mode = Mode::LineComment;
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    mode = Mode::BlockComment;
                    i += 1;
                }
                _ if !c.is_ascii_whitespace() => return true,
                _ => {}
            },
            Mode::LineComment => {
                if c == b'\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::BlockComment => {
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    mode = Mode::Normal;
                    i += 1;
                }
            }
            _ => unreachable!("has_code never enters a quote mode"),
        }
        i += 1;
    }
    false
}

/// Split a buffer into executable statement spans.
pub fn split(sql: &str) -> SplitOutput {
    let b = sql.as_bytes();

    if has_delimiter_directive(b) {
        let statements = trim_span(b, 0, b.len()).map_or_else(Vec::new, |s| vec![s]);
        return SplitOutput {
            statements,
            delimiter_detected: true,
        };
    }

    let mut statements = Vec::new();
    let mut mode = Mode::Normal;
    let mut seg_start = 0usize;
    let mut i = 0usize;

    while i < b.len() {
        let c = b[i];
        match mode {
            Mode::Normal => match c {
                b';' => {
                    if let Some(s) = trim_span(b, seg_start, i) {
                        statements.push(s);
                    }
                    seg_start = i + 1;
                }
                b'\'' => mode = Mode::Single,
                b'"' => mode = Mode::Double,
                b'`' => mode = Mode::Backtick,
                b'#' => mode = Mode::LineComment,
                b'-' if is_dash_comment(b, i) => {
                    mode = Mode::LineComment;
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    mode = Mode::BlockComment;
                    i += 1;
                }
                _ => {}
            },
            Mode::Single | Mode::Double | Mode::Backtick => {
                let q = match mode {
                    Mode::Single => b'\'',
                    Mode::Double => b'"',
                    _ => b'`',
                };
                if c == b'\\' && mode != Mode::Backtick {
                    i += 1;
                } else if c == q {
                    if b.get(i + 1) == Some(&q) {
                        i += 1;
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::LineComment => {
                if c == b'\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::BlockComment => {
                // MySQL block comments do not nest: the first `*/` closes.
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    mode = Mode::Normal;
                    i += 1;
                }
            }
        }
        i += 1;
    }

    // Trailing statement with no terminating semicolon.
    if let Some(s) = trim_span(b, seg_start, b.len()) {
        statements.push(s);
    }

    SplitOutput {
        statements,
        delimiter_detected: false,
    }
}

/// Return a copy of `sql` with every non-code byte replaced by a space:
/// string literals, quoted identifiers and comments are blanked out while
/// byte offsets stay identical to the original.
///
/// This is what makes keyword detection safe. Searching the raw text for
/// `LIMIT` would match a column named `` `limit` ``, the word inside
/// `'no limit'`, or a note in a comment. Searching the mask cannot.
pub fn mask_noncode(sql: &str) -> String {
    mask_impl(sql, false)
}

/// Like [`mask_noncode`], but keeps the *contents* of backtick-quoted
/// identifiers (the backticks themselves become spaces).
///
/// The linter needs this: `FROM \`users\`` must still yield the table name,
/// whereas auto-LIMIT detection must NOT see a column called `` `limit` ``.
/// Two different questions, so two different masks — sharing one scanner.
pub fn mask_keep_idents(sql: &str) -> String {
    mask_impl(sql, true)
}

fn mask_impl(sql: &str, keep_idents: bool) -> String {
    let b = sql.as_bytes();
    let mut out = vec![b' '; b.len()];
    let mut mode = Mode::Normal;
    let mut i = 0usize;

    while i < b.len() {
        let c = b[i];
        match mode {
            Mode::Normal => match c {
                b'\'' => mode = Mode::Single,
                b'"' => mode = Mode::Double,
                b'`' => mode = Mode::Backtick,
                b'#' => mode = Mode::LineComment,
                b'-' if is_dash_comment(b, i) => {
                    mode = Mode::LineComment;
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    mode = Mode::BlockComment;
                    i += 1;
                }
                _ => out[i] = c,
            },
            Mode::Single | Mode::Double | Mode::Backtick => {
                let q = match mode {
                    Mode::Single => b'\'',
                    Mode::Double => b'"',
                    _ => b'`',
                };
                if c == b'\\' && mode != Mode::Backtick {
                    i += 1;
                } else if c == q {
                    if b.get(i + 1) == Some(&q) {
                        i += 1;
                    } else {
                        mode = Mode::Normal;
                    }
                } else if keep_idents && mode == Mode::Backtick {
                    out[i] = c;
                }
            }
            Mode::LineComment => {
                if c == b'\n' {
                    mode = Mode::Normal;
                    out[i] = c; // keep newlines so line numbers survive
                }
            }
            Mode::BlockComment => {
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    mode = Mode::Normal;
                    i += 1;
                }
            }
        }
        i += 1;
    }
    // Every byte is ASCII space or an original ASCII byte; multi-byte UTF-8
    // sequences only ever appear inside masked regions, so this is valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| " ".repeat(sql.len()))
}

/// An unclosed string, identifier or block comment, and where it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unterminated {
    pub start: usize,
    pub kind: &'static str,
}

/// Report an unclosed quote or block comment, reusing the execution scanner so
/// the linter and the splitter can never disagree about what is "inside a
/// string". Returns `None` when the buffer is lexically balanced.
pub fn unterminated(sql: &str) -> Option<Unterminated> {
    let b = sql.as_bytes();
    let mut mode = Mode::Normal;
    let mut opened_at = 0usize;
    let mut i = 0usize;

    while i < b.len() {
        let c = b[i];
        match mode {
            Mode::Normal => match c {
                b'\'' => {
                    mode = Mode::Single;
                    opened_at = i;
                }
                b'"' => {
                    mode = Mode::Double;
                    opened_at = i;
                }
                b'`' => {
                    mode = Mode::Backtick;
                    opened_at = i;
                }
                b'#' => mode = Mode::LineComment,
                b'-' if is_dash_comment(b, i) => {
                    mode = Mode::LineComment;
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    mode = Mode::BlockComment;
                    opened_at = i;
                    i += 1;
                }
                _ => {}
            },
            Mode::Single | Mode::Double | Mode::Backtick => {
                let q = match mode {
                    Mode::Single => b'\'',
                    Mode::Double => b'"',
                    _ => b'`',
                };
                if c == b'\\' && mode != Mode::Backtick {
                    i += 1;
                } else if c == q {
                    if b.get(i + 1) == Some(&q) {
                        i += 1;
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            // A line comment is closed by end-of-input, so it is never unterminated.
            Mode::LineComment => {
                if c == b'\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::BlockComment => {
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    mode = Mode::Normal;
                    i += 1;
                }
            }
        }
        i += 1;
    }

    let kind = match mode {
        Mode::Single => "string literal",
        Mode::Double => "double-quoted string",
        Mode::Backtick => "quoted identifier",
        Mode::BlockComment => "block comment",
        _ => return None,
    };
    Some(Unterminated {
        start: opened_at,
        kind,
    })
}

/// Index of the statement containing `cursor`, using the same scan as
/// execution. Falls back to the statement the cursor sits just after, so
/// clicking at the end of a line still selects that line's statement.
pub fn statement_at(spans: &[StatementSpan], cursor: usize) -> Option<usize> {
    if spans.is_empty() {
        return None;
    }
    for (idx, s) in spans.iter().enumerate() {
        if cursor >= s.start && cursor <= s.end {
            return Some(idx);
        }
    }
    // Between statements: attach to the preceding one, else the first.
    spans
        .iter()
        .enumerate()
        .rfind(|(_, s)| s.end < cursor)
        .map(|(i, _)| i)
        .or(Some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sql: &str) -> Vec<&str> {
        split(sql)
            .statements
            .iter()
            .map(|s| &sql[s.start..s.end])
            .collect()
    }

    #[test]
    fn splits_plain_statements() {
        assert_eq!(
            texts("SELECT 1; SELECT 2; SELECT 3"),
            vec!["SELECT 1", "SELECT 2", "SELECT 3"]
        );
    }

    #[test]
    fn semicolon_inside_single_quoted_string_is_not_a_boundary() {
        assert_eq!(
            texts("SELECT 'a;b'; SELECT 2"),
            vec!["SELECT 'a;b'", "SELECT 2"]
        );
    }

    #[test]
    fn semicolon_inside_double_quoted_string_is_not_a_boundary() {
        assert_eq!(
            texts(r#"SELECT "a;b"; SELECT 2"#),
            vec![r#"SELECT "a;b""#, "SELECT 2"]
        );
    }

    #[test]
    fn semicolon_inside_backtick_identifier_is_not_a_boundary() {
        assert_eq!(
            texts("SELECT `we;ird` FROM t; SELECT 2"),
            vec!["SELECT `we;ird` FROM t", "SELECT 2"]
        );
    }

    #[test]
    fn backslash_escaped_quote_does_not_end_the_string() {
        assert_eq!(
            texts(r#"SELECT 'it\'s; fine'; SELECT 2"#),
            vec![r#"SELECT 'it\'s; fine'"#, "SELECT 2"]
        );
    }

    #[test]
    fn doubled_quote_does_not_end_the_string() {
        assert_eq!(
            texts("SELECT 'it''s; fine'; SELECT 2"),
            vec!["SELECT 'it''s; fine'", "SELECT 2"]
        );
    }

    #[test]
    fn backslash_is_literal_inside_backticks() {
        // `a\` is a complete identifier: the backslash escapes nothing, so the
        // backtick right after it closes. The `;` that follows is a boundary.
        assert_eq!(
            texts(r"SELECT `a\`; SELECT 2"),
            vec![r"SELECT `a\`", "SELECT 2"]
        );
    }

    #[test]
    fn semicolon_in_dash_comment_is_not_a_boundary() {
        assert_eq!(
            texts("SELECT 1 -- ; not a split\n; SELECT 2"),
            vec!["SELECT 1 -- ; not a split", "SELECT 2"]
        );
    }

    #[test]
    fn semicolon_in_hash_comment_is_not_a_boundary() {
        assert_eq!(
            texts("SELECT 1 # ; nope\n; SELECT 2"),
            vec!["SELECT 1 # ; nope", "SELECT 2"]
        );
    }

    #[test]
    fn dash_dash_without_trailing_space_is_not_a_comment() {
        // MySQL requires whitespace after `--`. `5--3` is 5 minus negative 3.
        assert_eq!(
            texts("SELECT 5--3; SELECT 2"),
            vec!["SELECT 5--3", "SELECT 2"]
        );
    }

    #[test]
    fn semicolon_in_block_comment_is_not_a_boundary() {
        assert_eq!(
            texts("SELECT 1 /* ; still one */ ; SELECT 2"),
            vec!["SELECT 1 /* ; still one */", "SELECT 2"]
        );
    }

    #[test]
    fn block_comments_do_not_nest() {
        // MySQL closes at the FIRST `*/`, so the `;` after it IS a boundary
        // and the trailing `*/` belongs to the second statement.
        assert_eq!(
            texts("SELECT 1 /* outer /* inner */ ; SELECT 2"),
            vec!["SELECT 1 /* outer /* inner */", "SELECT 2"]
        );
    }

    #[test]
    fn trailing_semicolon_produces_no_empty_statement() {
        assert_eq!(texts("SELECT 1;"), vec!["SELECT 1"]);
        assert_eq!(texts("SELECT 1;   \n  "), vec!["SELECT 1"]);
    }

    #[test]
    fn empty_statements_are_dropped() {
        assert_eq!(texts(";;; SELECT 1 ;;\n;"), vec!["SELECT 1"]);
        assert!(split("").statements.is_empty());
        assert!(split("   \n\t ").statements.is_empty());
    }

    #[test]
    fn comment_only_buffer_is_not_a_statement() {
        assert!(split("-- just a note\n").statements.is_empty());
        assert!(split("/* nothing here */").statements.is_empty());
        assert!(split("# hash note").statements.is_empty());
    }

    #[test]
    fn leading_comment_stays_attached_to_its_statement() {
        assert_eq!(
            texts("-- explain\nSELECT 1; SELECT 2"),
            vec!["-- explain\nSELECT 1", "SELECT 2"]
        );
    }

    #[test]
    fn delimiter_directive_bails_out_to_a_single_span() {
        let sql = "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END $$\nDELIMITER ;";
        let out = split(sql);
        assert!(out.delimiter_detected);
        assert_eq!(out.statements.len(), 1);
        assert_eq!(&sql[out.statements[0].start..out.statements[0].end], sql);
    }

    #[test]
    fn the_word_delimiter_in_a_string_does_not_trigger_bailout() {
        let out = split("SELECT 'DELIMITER $$'; SELECT 2");
        assert!(!out.delimiter_detected);
        assert_eq!(out.statements.len(), 2);
    }

    #[test]
    fn delimiter_as_a_column_name_does_not_trigger_bailout() {
        // Not at the start of a line, so it is an identifier, not a directive.
        let out = split("SELECT delimiter_col FROM t; SELECT 2");
        assert!(!out.delimiter_detected);
        assert_eq!(out.statements.len(), 2);
    }

    #[test]
    fn offsets_are_byte_exact_with_multibyte_text() {
        let sql = "SELECT 'héllo → ;'; SELECT 2";
        let out = split(sql);
        assert_eq!(out.statements.len(), 2);
        // Slicing by the reported offsets must not panic and must round-trip.
        assert_eq!(
            &sql[out.statements[0].start..out.statements[0].end],
            "SELECT 'héllo → ;'"
        );
        assert_eq!(
            &sql[out.statements[1].start..out.statements[1].end],
            "SELECT 2"
        );
    }

    #[test]
    fn unterminated_string_consumes_the_rest() {
        // Better to hand MySQL one broken statement than to mis-split it.
        assert_eq!(
            texts("SELECT 'oops; SELECT 2"),
            vec!["SELECT 'oops; SELECT 2"]
        );
    }

    #[test]
    fn unterminated_block_comment_consumes_the_rest() {
        assert!(split("SELECT 1 /* oops; SELECT 2").statements.len() == 1);
    }

    #[test]
    fn statement_at_finds_the_cursor_statement() {
        let sql = "SELECT 1; SELECT 2; SELECT 3";
        let spans = split(sql).statements;
        assert_eq!(statement_at(&spans, 0), Some(0));
        assert_eq!(statement_at(&spans, 8), Some(0));
        assert_eq!(statement_at(&spans, 12), Some(1));
        assert_eq!(statement_at(&spans, 27), Some(2));
        // Cursor sitting on the separator attaches to the preceding statement.
        assert_eq!(statement_at(&spans, 9), Some(0));
    }
    #[test]
    fn mask_blanks_strings_comments_and_identifiers() {
        let sql = "SELECT `limit` FROM t WHERE s = 'limit' -- limit\nAND x = 1";
        let m = mask_noncode(sql);
        assert_eq!(m.len(), sql.len());
        // Only the real keyword-free code survives; no stray "limit" remains.
        assert!(!m.to_uppercase().contains("LIMIT"));
        assert!(m.contains("SELECT"));
        assert!(m.contains("FROM t WHERE s ="));
        assert!(m.contains("AND x = 1"));
    }

    #[test]
    fn mask_keeps_a_real_top_level_limit() {
        let sql = "SELECT * FROM t LIMIT 10";
        assert!(mask_noncode(sql).contains("LIMIT"));
    }

    #[test]
    fn mask_preserves_offsets_with_multibyte_content() {
        let sql = "SELECT 'héllo → x' AS a";
        let m = mask_noncode(sql);
        assert_eq!(m.len(), sql.len());
        assert!(m.contains("SELECT"));
        assert!(m.contains("AS a"));
    }
    #[test]
    fn detects_an_unterminated_string() {
        let u = unterminated("SELECT 'oops").unwrap();
        assert_eq!(u.start, 7);
        assert_eq!(u.kind, "string literal");
    }

    #[test]
    fn detects_an_unterminated_block_comment_and_identifier() {
        assert_eq!(unterminated("SELECT 1 /* x").unwrap().kind, "block comment");
        assert_eq!(unterminated("SELECT `x").unwrap().kind, "quoted identifier");
    }

    #[test]
    fn balanced_text_reports_nothing_unterminated() {
        assert!(unterminated("SELECT 'a', \"b\", `c` /* d */ -- e\n").is_none());
        assert!(unterminated("SELECT 'it''s'").is_none());
        assert!(unterminated(r"SELECT 'it\'s'").is_none());
        // A line comment running to EOF is closed, not unterminated.
        assert!(unterminated("SELECT 1 -- trailing").is_none());
    }

    #[test]
    fn mask_keep_idents_preserves_backticked_names() {
        let sql = "SELECT * FROM `my table` WHERE s = 'x'";
        let m = mask_keep_idents(sql);
        assert_eq!(m.len(), sql.len());
        assert!(m.contains("my table"), "identifier lost: {m:?}");
        assert!(!m.contains('x'), "string literal leaked: {m:?}");
        // The strict mask must still blank it, or auto-LIMIT would misfire.
        assert!(!mask_noncode(sql).contains("my table"));
    }
}
