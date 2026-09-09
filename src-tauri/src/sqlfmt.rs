// Laying SQL out so it can be read.
//
// # Why this exists
//
// MySQL stores a **view** as a normalised one-liner. Whatever you typed, what
// comes back from `SHOW CREATE VIEW` is a single line several hundred
// characters wide, and "Examine definition…" put exactly that in a tab. The
// answer was correct and unreadable, which for a question like "what does this
// view actually select?" is close to no answer at all.
//
// Routines are the opposite: `SHOW CREATE PROCEDURE` hands the body back as it
// was written, newlines and all. So this must **not** reformat everything it
// is given — a routine someone laid out by hand is already better than
// anything here would produce.
//
// # The two rules it obeys
//
//   1. **It only acts on dense text.** If every line is already narrower than
//      [`WIDE`], the input is returned untouched. That is what separates the
//      one-lined view from the hand-formatted routine, without needing to know
//      which one it was handed.
//   2. **It only moves whitespace.** The output is re-lexed and compared with
//      the input token for token; if they differ at all, the original is
//      returned. A formatter that changes what a definition *means* is a bug
//      that ends in someone running the result, so it fails closed rather than
//      trusting the rules below to be complete.
//
// Both are cheap, and together they mean the worst case is "no worse than
// before" rather than "subtly wrong SQL in an editor".

/// Lines wider than this are taken as evidence that nothing laid this out.
const WIDE: usize = 120;

/// Two spaces, matching the SQL this app generates elsewhere.
const INDENT: &str = "  ";

/// A parenthesised span wider than this gets lines of its own. MySQL wraps a
/// view's whole `FROM` clause — every join, every condition — in one pair of
/// parentheses, and leaving that inline puts most of the definition back on a
/// single line.
const LONG_SPAN: usize = 60;

/// One lexical unit. Whitespace is deliberately **not** a token: whitespace is
/// the only thing this module is allowed to change, so leaving it out of the
/// comparison is what makes the comparison meaningful.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok<'a> {
    /// An identifier, keyword or number.
    Word(&'a str),
    /// A quoted run — `'…'`, `"…"` or `` `…` `` — kept exactly as it came.
    Quoted(&'a str),
    /// `-- …`, `# …` or `/* … */`, likewise kept exactly.
    Comment(&'a str),
    /// An operator or a separator.
    Punct(&'a str),
}

impl Tok<'_> {
    fn text(&self) -> &str {
        match self {
            Tok::Word(s) | Tok::Quoted(s) | Tok::Comment(s) | Tok::Punct(s) => s,
        }
    }
    fn upper(&self) -> String {
        match self {
            Tok::Word(s) => s.to_ascii_uppercase(),
            _ => String::new(),
        }
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Operators that must stay together. Longest first, since the scan takes the
/// first that matches.
const OPERATORS: &[&str] = &[
    "->>", "<=>", "<>", "<=", ">=", "!=", ":=", "||", "&&", "<<", ">>", "->",
];

/// Split SQL into tokens, and record which of them the author wrote with no
/// space in front.
///
/// That adjacency is needed for exactly one character. `@` joins a definer's
/// two halves (`` `root`@`localhost` ``) and introduces a session variable
/// (`SET @x = 0`); the difference is not in the tokens around it, only in
/// whether the author left a space. Spacing a definer's `@` is invalid SQL, so
/// this is not a cosmetic question.
///
/// Never fails: an unterminated string or comment runs to the end of the input
/// and is returned as one token, which keeps the round-trip comparison honest
/// about malformed input rather than panicking on it.
fn scan(sql: &str) -> (Vec<Tok<'_>>, Vec<bool>) {
    let b = sql.as_bytes();
    let n = sql.len();
    let mut i = 0;
    let mut out = Vec::new();
    let mut glued = Vec::new();
    // Where the previous token ended, so the next one knows whether anything
    // separated them.
    let mut end_of_last = 0;

    macro_rules! emit {
        ($tok:expr, $from:expr, $to:expr) => {{
            glued.push($from == end_of_last && !out.is_empty());
            out.push($tok);
            end_of_last = $to;
        }};
    }

    while i < n {
        let c = b[i];

        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Comments, before the operators that start them.
        if c == b'#' || (c == b'-' && b.get(i + 1) == Some(&b'-')) {
            let end = sql[i..].find('\n').map_or(n, |k| i + k);
            emit!(Tok::Comment(&sql[i..end]), i, end);
            i = end;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = sql[i + 2..].find("*/").map_or(n, |k| i + 2 + k + 2);
            emit!(Tok::Comment(&sql[i..end]), i, end);
            i = end;
            continue;
        }

        // Quoted runs. All three closers double themselves to escape, and the
        // two string quotes also honour a backslash.
        if c == b'\'' || c == b'"' || c == b'`' {
            let quote = c;
            let backslash = quote != b'`';
            let mut j = i + 1;
            loop {
                if j >= n {
                    break;
                }
                if backslash && b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if b[j] == quote {
                    if b.get(j + 1) == Some(&quote) {
                        j += 2;
                        continue;
                    }
                    j += 1;
                    break;
                }
                j += 1;
            }
            let end = j.min(n);
            emit!(Tok::Quoted(&sql[i..end]), i, end);
            i = end;
            continue;
        }

        // Numbers are lexed whole — including the exponent. `1.5e-3` split
        // around its `-` would be re-spaced into `1.5e - 3`, which is a
        // different expression, and exactly the kind of change rule 2 exists
        // to catch. Better not to make it.
        if c.is_ascii_digit() {
            let mut j = i;
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < n && b[j] == b'.' && b.get(j + 1).is_some_and(|d| d.is_ascii_digit()) {
                j += 1;
                while j < n && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j < n && (b[j] | 0x20) == b'e' {
                let mut k = j + 1;
                if k < n && (b[k] == b'+' || b[k] == b'-') {
                    k += 1;
                }
                if k < n && b[k].is_ascii_digit() {
                    while k < n && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    j = k;
                }
            }
            // A digit run that runs straight into letters is an identifier
            // (`1st_try`) or a hex literal, so let the word scan take it.
            if !(j < n && is_word_byte(b[j])) {
                emit!(Tok::Word(&sql[i..j]), i, j);
                i = j;
                continue;
            }
        }

        if is_word_byte(c) {
            let mut j = i;
            while j < n && is_word_byte(b[j]) {
                j += 1;
            }
            emit!(Tok::Word(&sql[i..j]), i, j);
            i = j;
            continue;
        }

        if let Some(op) = OPERATORS.iter().find(|op| sql[i..].starts_with(**op)) {
            emit!(Tok::Punct(&sql[i..i + op.len()]), i, i + op.len());
            i += op.len();
            continue;
        }

        // A single byte, and the only remaining case. ASCII by elimination:
        // everything above 0x7f was taken by the word scan.
        emit!(Tok::Punct(&sql[i..i + 1]), i, i + 1);
        i += 1;
    }

    (out, glued)
}

/// The tokens alone, for the comparison that decides whether a rewrite is safe.
fn lex(sql: &str) -> Vec<Tok<'_>> {
    scan(sql).0
}

/// How wide a parenthesised span would be if it stayed on one line, counting
/// one space between tokens. Stops as soon as the answer is "wider than
/// `cap`", which is all the caller asks.
fn paren_width(toks: &[Tok<'_>], open: usize, cap: usize) -> usize {
    let mut depth = 0usize;
    let mut width = 0usize;
    for tok in &toks[open..] {
        width += tok.text().chars().count() + 1;
        match tok {
            Tok::Punct("(") => depth += 1,
            Tok::Punct(")") => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        if width > cap {
            break;
        }
    }
    width
}

/// Clause keywords that start their own line.
const CLAUSES: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "VALUES",
    "RETURNING",
    "WINDOW",
];

/// Statement keywords that start their own line inside a routine body.
const STATEMENTS: &[&str] = &[
    "DECLARE", "SET", "IF", "CALL", "RETURN", "OPEN", "CLOSE", "FETCH", "LEAVE", "ITERATE",
    "INSERT", "UPDATE", "DELETE", "REPLACE", "CREATE", "DROP", "TRUNCATE", "START", "COMMIT",
    "ROLLBACK", "SIGNAL", "RESIGNAL", "WHILE",
];

/// The words that can precede `JOIN`. Every one of them is also a function or
/// a column name somewhere, which is why the break is decided by looking ahead
/// for the `JOIN` rather than by the word alone: `LEFT(name, 3)` must not open
/// a new line.
const JOIN_QUALIFIERS: &[&str] = &[
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "CROSS",
    "FULL",
    "NATURAL",
    "STRAIGHT_JOIN",
];

/// What an open block is, so `END` knows what it is closing.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Block {
    /// `BEGIN`, `IF … THEN`, `LOOP`, `REPEAT` — a block whose `END` belongs on
    /// its own line.
    Body,
    /// `CASE` used as an expression, in the middle of a line. Its `END` closes
    /// an expression, not a block, so it must not break the line or dedent.
    InlineCase,
}

struct Layout {
    out: String,
    line: String,
    indent: usize,
    /// Indent of the line being built, fixed when the line was started so a
    /// dedent mid-line cannot move text that is already there.
    line_indent: usize,
    /// Parentheses that opened a line break, innermost last. `false` means the
    /// parenthesis is inline, which suppresses every break inside it — a
    /// function's arguments are not a place for clause layout.
    parens: Vec<bool>,
    blocks: Vec<Block>,
}

impl Layout {
    fn new() -> Self {
        Layout {
            out: String::new(),
            line: String::new(),
            indent: 0,
            line_indent: 0,
            parens: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// True when no inline parenthesis is open — the only place clause and
    /// statement breaks are allowed.
    fn breakable(&self) -> bool {
        self.parens.iter().all(|broke| *broke)
    }

    fn newline(&mut self, indent: usize) {
        if !self.line.is_empty() {
            for _ in 0..self.line_indent {
                self.out.push_str(INDENT);
            }
            self.out.push_str(self.line.trim_end());
            self.out.push('\n');
            self.line.clear();
        }
        self.line_indent = indent;
    }

    /// One empty line, to separate top-level statements. Never two, and never
    /// one at the very top.
    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn push(&mut self, text: &str, space: bool) {
        if space && !self.line.is_empty() {
            self.line.push(' ');
        }
        self.line.push_str(text);
    }

    fn finish(mut self) -> String {
        self.newline(0);
        self.out
    }
}

/// Whether a space belongs between the previous token and this one.
///
/// `glued` is what the author wrote — whether these two were adjacent in the
/// source — and matters only for `@`, which joins the halves of a definer and
/// introduces a session variable with the same character.
fn wants_space(prev: Option<&Tok<'_>>, tok: &Tok<'_>, glued: bool) -> bool {
    let Some(prev) = prev else { return false };

    // `.` binds its neighbours unconditionally: `db`.`t` is one name and there
    // is no other reading of it.
    if matches!(prev, Tok::Punct(".")) || matches!(tok, Tok::Punct(".")) {
        return false;
    }
    if matches!(prev, Tok::Punct("@")) || matches!(tok, Tok::Punct("@")) {
        return !glued;
    }
    if matches!(tok, Tok::Punct(",") | Tok::Punct(";") | Tok::Punct(")")) {
        return false;
    }
    if matches!(prev, Tok::Punct("(")) {
        return false;
    }
    // `count(*)` but `CREATE TABLE `t` (…)`, and the difference is not in the
    // tokens: it is whether the author wrote a space. A keyword list gets this
    // right until it meets the keyword nobody listed, and the source already
    // knows the answer.
    if matches!(tok, Tok::Punct("(")) {
        return !glued;
    }
    true
}

/// Lay out a token stream. Pure whitespace decisions — every token is emitted,
/// in order, exactly as it was lexed.
fn layout(toks: &[Tok<'_>], glued: &[bool]) -> String {
    // Trailing `$` is stripped for **matching only** — the token itself is
    // still emitted exactly as it came. `$` is a word byte because MySQL
    // identifiers may contain one, which makes `END$$` a single word and hides
    // the block closer inside a routine written with a custom delimiter. An
    // identifier genuinely ending in `$` would be mis-indented and nothing
    // more; rule 2 still guarantees the SQL is unchanged.
    let words: Vec<String> = toks
        .iter()
        .map(|t| t.upper().trim_end_matches('$').to_string())
        .collect();
    let word_at = |i: usize| words.get(i).map(String::as_str).unwrap_or("");
    let is_open_paren = |i: usize| matches!(toks.get(i), Some(Tok::Punct("(")));

    let mut f = Layout::new();
    // `WHERE`/`ON`/`HAVING` are the only places `AND` and `OR` deserve a line
    // of their own; in `IF a AND b THEN` they do not.
    let mut in_condition = false;
    // `BETWEEN x AND y` owns the next `AND`, which is not a conjunction.
    let mut pending_between = false;
    // Set by the block openers, whose next token starts a statement.
    let mut force_break = false;
    // `END IF` / `END LOOP` — the second word is a closer, not an opener.
    let mut after_end = false;

    for (i, tok) in toks.iter().enumerate() {
        let w = word_at(i);
        let next = word_at(i + 1);
        let inline_case = f.blocks.last() == Some(&Block::InlineCase);
        let mut space = wants_space(
            i.checked_sub(1).and_then(|k| toks.get(k)),
            tok,
            glued.get(i).copied().unwrap_or(false),
        );

        if force_break && !matches!(tok, Tok::Punct(")")) {
            f.newline(f.indent);
            force_break = false;
        }

        // ------------------------------------------------------- structure
        match tok {
            Tok::Punct("(") => {
                // A parenthesis gets lines of its own when it holds a query,
                // or when it is simply too wide to read on one. Everything
                // else — a function call, a type's length, a short `IN` list —
                // stays where it started.
                let opens = next == "SELECT"
                    || next == "WITH"
                    || paren_width(toks, i, LONG_SPAN) > LONG_SPAN;
                f.push("(", space);
                f.parens.push(opens);
                if opens {
                    f.indent += 1;
                    f.newline(f.indent);
                }
                continue;
            }
            Tok::Punct(")") => {
                if f.parens.pop() == Some(true) {
                    f.indent = f.indent.saturating_sub(1);
                    f.newline(f.indent);
                    space = false;
                }
                f.push(")", space);
                continue;
            }
            Tok::Punct(",") if f.breakable() => {
                f.push(",", false);
                // Inside a broken parenthesis the items are already indented by
                // it; a select list's are not, and hang under their clause.
                let hang = usize::from(f.parens.last() != Some(&true));
                f.newline(f.indent + hang);
                continue;
            }
            Tok::Punct(";") => {
                f.push(";", false);
                f.newline(f.indent);
                in_condition = false;
                // A script is easier to read as separated statements than as a
                // wall. Only at the top level: the `;`s inside a routine body
                // belong to one statement and must not be spread apart.
                if f.indent == 0 && f.blocks.is_empty() && i + 1 < toks.len() {
                    f.blank();
                }
                continue;
            }
            Tok::Comment(_) => {
                // A line comment swallows the rest of its line, so anything
                // after it must start a new one.
                f.push(tok.text(), space);
                if !tok.text().starts_with("/*") {
                    f.newline(f.indent);
                }
                continue;
            }
            _ => {}
        }

        if !matches!(tok, Tok::Word(_)) {
            f.push(tok.text(), space);
            continue;
        }

        // ---------------------------------------------------------- blocks
        //
        // `END IF`, `END WHILE`, `END LOOP`: the second word is part of the
        // closer. Without this it breaks the line again as a statement keyword
        // and leaves a bare `IF;` behind.
        let closing = after_end;
        let opens_block = !closing && !inline_case;
        match w {
            "BEGIN" if f.breakable() => {
                f.newline(f.indent);
                f.push(tok.text(), false);
                f.blocks.push(Block::Body);
                f.indent += 1;
                force_break = true;
                after_end = false;
                continue;
            }
            "CASE" => {
                // A `CASE` that starts a line is a statement; one that appears
                // mid-expression is a value, and its `END` must not dedent.
                f.blocks.push(if f.line.is_empty() {
                    Block::Body
                } else {
                    Block::InlineCase
                });
                f.push(tok.text(), space);
                after_end = false;
                continue;
            }
            "END" => {
                if f.blocks.pop() == Some(Block::InlineCase) {
                    f.push(tok.text(), space);
                } else {
                    f.indent = f.indent.saturating_sub(1);
                    f.newline(f.indent);
                    f.push(tok.text(), false);
                }
                after_end = true;
                continue;
            }
            "THEN" | "DO" if opens_block && f.breakable() => {
                f.push(tok.text(), space);
                f.indent += 1;
                force_break = true;
                continue;
            }
            "ELSE" if opens_block && f.breakable() => {
                f.indent = f.indent.saturating_sub(1);
                f.newline(f.indent);
                f.push(tok.text(), false);
                f.indent += 1;
                force_break = true;
                continue;
            }
            "ELSEIF" if opens_block && f.breakable() => {
                f.indent = f.indent.saturating_sub(1);
                f.newline(f.indent);
                f.push(tok.text(), false);
                continue;
            }
            "LOOP" | "REPEAT" if opens_block && f.breakable() => {
                f.newline(f.indent);
                f.push(tok.text(), false);
                f.blocks.push(Block::Body);
                f.indent += 1;
                force_break = true;
                continue;
            }
            "UNTIL" if f.breakable() => {
                f.newline(f.indent.saturating_sub(1));
                f.push(tok.text(), false);
                after_end = false;
                continue;
            }
            _ => {}
        }
        after_end = false;

        // --------------------------------------------------------- clauses
        if f.breakable() {
            let breaks = CLAUSES.contains(&w)
                || (matches!(w, "GROUP" | "ORDER" | "PARTITION") && next == "BY")
                || (w == "JOIN" && !JOIN_QUALIFIERS.contains(&word_at(i.wrapping_sub(1))))
                || (JOIN_QUALIFIERS.contains(&w) && (next == "JOIN" || word_at(i + 2) == "JOIN"))
                // A statement keyword that is immediately followed by `(` is a
                // function of the same name, and starts nothing.
                || (STATEMENTS.contains(&w) && !closing && !is_open_paren(i + 1));

            if breaks {
                f.newline(f.indent);
                space = false;
                in_condition = false;
            } else if matches!(w, "AND" | "OR") && in_condition && !pending_between {
                f.newline(f.indent + 1);
                space = false;
            }

            if matches!(w, "WHERE" | "ON" | "HAVING") {
                in_condition = true;
            }
        }

        pending_between = if w == "BETWEEN" {
            true
        } else {
            pending_between && w != "AND"
        };

        f.push(tok.text(), space);
    }

    f.finish()
}

/// Reformat a definition that arrived as one dense line, and leave everything
/// else exactly as it was.
///
/// Returns the input unchanged if the rewrite would have changed any token —
/// see rule 2 at the top of this file.
pub fn tidy(sql: &str) -> String {
    if sql.lines().all(|l| l.chars().count() <= WIDE) {
        return sql.to_string();
    }
    format(sql).unwrap_or_else(|| sql.to_string())
}

/// Lay SQL out **because the user asked**, however wide its lines already are.
///
/// This is [`tidy`] without rule 1. Rule 2 still holds, and here it is the
/// whole interface: `None` means the rewrite would have changed a token, so the
/// caller can say "I could not format this" instead of silently doing nothing —
/// or, far worse, handing back SQL that no longer means what it did.
///
/// # `DELIMITER` lines are copied, not formatted
///
/// `DELIMITER` is a **client** directive that owns its whole line; the server
/// has never understood it. Running it through the layout is not merely
/// pointless, it is destructive: `END$$` followed by `DELIMITER ;` lays out as
/// `END$$ DELIMITER;`, which is token-for-token identical — so rule 2 cannot
/// see it — and no longer runs. So the text is split on those lines, each run
/// between them is laid out on its own, and the directives are carried across
/// verbatim.
pub fn format(sql: &str) -> Option<String> {
    let mut out = String::new();
    for chunk in chunks(sql) {
        match chunk {
            Chunk::Directive(line) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(line.trim());
                out.push('\n');
            }
            Chunk::Sql(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                let (before, glued) = scan(text);
                let laid = layout(&before, &glued);
                // Per run rather than over the whole document: a run that
                // cannot be laid out safely should fail the whole request, and
                // comparing here says which one.
                if lex(&laid) != before {
                    return None;
                }
                out.push_str(laid.trim_end());
                out.push('\n');
            }
        }
    }
    Some(out)
}

enum Chunk<'a> {
    /// A `DELIMITER …` line, to be reproduced as it stands.
    Directive(&'a str),
    /// Everything else.
    Sql(&'a str),
}

/// Split on `DELIMITER` directive lines.
///
/// The rule is `split.rs`'s, deliberately: `DELIMITER` as the first
/// non-whitespace on a line, followed by whitespace and a token. Two modules
/// disagreeing about what a directive is would be a bug nobody could see from
/// either one.
///
/// This does **not** track strings or comments, which `split.rs` does. The
/// consequence is bounded and in the safe direction: the word `DELIMITER`
/// starting a line inside a string literal or block comment would split a run
/// in two, and the token comparison in [`format`] then rejects the whole thing.
/// Refusing to format is a worse outcome than formatting; it is not a wrong
/// outcome.
fn chunks(sql: &str) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut at = 0;

    for line in sql.split_inclusive('\n') {
        if is_delimiter_directive(line) {
            if at > run_start {
                out.push(Chunk::Sql(&sql[run_start..at]));
            }
            out.push(Chunk::Directive(line));
            run_start = at + line.len();
        }
        at += line.len();
    }
    if at > run_start {
        out.push(Chunk::Sql(&sql[run_start..at]));
    }
    out
}

fn is_delimiter_directive(line: &str) -> bool {
    let t = line.trim_start();
    let Some(rest) = t
        .get(..9)
        .filter(|k| k.eq_ignore_ascii_case("DELIMITER"))
        .map(|_| &t[9..])
    else {
        return false;
    };
    // Whitespace after the keyword, or `DELIMITERS` would qualify — and then a
    // token, since a bare `DELIMITER` sets nothing and is not a directive.
    rest.starts_with([' ', '\t']) && !rest.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape MySQL hands back for a view: one line, normalised,
    /// several hundred characters wide.
    pub(super) const VIEW_SAMPLE: &str = "CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`localhost` SQL SECURITY DEFINER VIEW `poc`.`user_totals` AS select `u`.`id` AS `id`,`u`.`email` AS `email`,count(`o`.`id`) AS `orders`,coalesce(sum(`o`.`total`),0) AS `spent` from (`poc`.`users` `u` left join `poc`.`orders` `o` on((`o`.`user_id` = `u`.`id`))) where `u`.`id` is not null group by `u`.`id`,`u`.`email` order by `spent` desc";

    pub(super) const ROUTINE_SAMPLE: &str = "CREATE PROCEDURE `poc`.`top`(IN n INT) BEGIN DECLARE done INT DEFAULT 0; SET @x = 0; IF n > 0 THEN SELECT id, total FROM orders WHERE total > n ORDER BY total DESC LIMIT n; ELSE SELECT 0; END IF; WHILE @x < n DO SET @x = @x + 1; END WHILE; END";

    // ------------------------------------------------------- format(), the command

    /// The corruption this function's `DELIMITER` handling exists to prevent.
    /// `END$$ DELIMITER;` is token-identical to the input, so rule 2 cannot see
    /// it, and it does not run.
    #[test]
    fn a_delimiter_directive_keeps_its_own_line() {
        let script = "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$\nDELIMITER ;\n";
        let out = format(script).expect("formattable");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.first(), Some(&"DELIMITER $$"), "{out}");
        assert_eq!(lines.last(), Some(&"DELIMITER ;"), "{out}");
        assert!(
            !out.contains("DELIMITER;"),
            "the closing directive was swallowed into the statement:\n{out}"
        );
    }

    /// `$` is a word byte, so `END$$` lexes as one word and the block closer
    /// hides inside it — which left the whole routine over-indented by one.
    #[test]
    fn a_routine_written_with_a_custom_delimiter_closes_its_block() {
        let out = format("DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$\nDELIMITER ;\n")
            .expect("formattable");
        assert!(
            out.contains("\nEND$$\n"),
            "END$$ did not return to column zero:\n{out}"
        );
    }

    #[test]
    fn delimiters_is_not_a_delimiter_directive() {
        assert!(!is_delimiter_directive("DELIMITERS $$\n"));
        // A bare one sets nothing.
        assert!(!is_delimiter_directive("DELIMITER\n"));
        assert!(!is_delimiter_directive("SELECT delimiter FROM t;\n"));
        assert!(is_delimiter_directive("DELIMITER $$\n"));
        assert!(is_delimiter_directive("  delimiter ;\n"));
    }

    /// A column that happens to be called `delimiter` is not a directive, and
    /// must come through as ordinary SQL.
    #[test]
    fn a_column_named_delimiter_is_left_alone() {
        let out = format("select delimiter from t where delimiter = 1;").expect("formattable");
        assert!(out.contains("select delimiter"), "{out}");
        assert_eq!(
            lex(&out),
            lex("select delimiter from t where delimiter = 1;")
        );
    }

    /// Rule 1 is what `format` drops: an already-narrow statement is laid out
    /// anyway, because the user asked for it.
    #[test]
    fn format_lays_out_narrow_text_that_tidy_would_leave_alone() {
        let sql = "select a,b from t where a=1;";
        assert_eq!(tidy(sql), sql, "tidy should not touch a narrow line");
        let out = format(sql).expect("formattable");
        assert_ne!(out, sql);
        assert!(out.contains("\nfrom t"), "{out}");
    }

    /// Rule 2 is what it keeps, and here it is the entire interface.
    #[test]
    fn format_refuses_rather_than_changing_what_the_sql_means() {
        // An unterminated string swallows the rest of the input as one token,
        // which is exactly the input a formatter must not guess at.
        let out = format("select 'unterminated");
        assert!(
            out.is_none() || lex(&out.unwrap()) == lex("select 'unterminated"),
            "format changed a token"
        );
    }

    #[test]
    fn top_level_statements_are_separated_and_a_routine_body_is_not() {
        let out = format("select 1; select 2;").expect("formattable");
        assert!(
            out.contains(";\n\nselect 2"),
            "no blank line between statements:\n{out}"
        );

        let body = format(ROUTINE_SAMPLE).expect("formattable");
        assert!(
            !body.contains("\n\n"),
            "a routine body was spread apart:\n{body}"
        );
    }

    /// Every sample this module has, through the entry point the command uses.
    #[test]
    fn format_preserves_every_token_of_every_sample() {
        for sample in [VIEW_SAMPLE, ROUTINE_SAMPLE, "select a,b from t;", ""] {
            let out = format(sample).unwrap_or_else(|| panic!("refused: {sample}"));
            assert_eq!(lex(&out), lex(sample), "tokens changed for: {sample}");
        }
    }

    /// Rule 2, on everything this module is ever asked to lay out. Nothing
    /// below matters if the output is not the same SQL.
    /// Lay out, for tests that want the result whether or not `tidy` would
    /// have chosen to reformat this input.
    fn laid_out(sql: &str) -> String {
        let (toks, glued) = scan(sql);
        layout(&toks, &glued)
    }

    fn assert_tokens_preserved(input: &str) {
        let out = laid_out(input);
        assert_eq!(
            lex(&out),
            lex(input),
            "layout changed the tokens\n--- in ---\n{input}\n--- out ---\n{out}"
        );
    }

    #[test]
    fn a_view_definition_gains_lines_without_gaining_or_losing_a_token() {
        assert_tokens_preserved(VIEW_SAMPLE);
        let out = tidy(VIEW_SAMPLE);
        assert!(out.lines().count() > 8, "still dense:\n{out}");
        assert!(
            out.lines().all(|l| l.chars().count() <= WIDE),
            "still has a wide line:\n{out}"
        );
        // The clauses are where you would look for them.
        assert!(
            out.contains("\nfrom ("),
            "from did not start a line:\n{out}"
        );
        assert!(
            out.contains("\nwhere "),
            "where did not start a line:\n{out}"
        );
        assert!(
            out.contains("\ngroup by "),
            "group by did not start a line:\n{out}"
        );
        assert!(
            out.contains("\norder by "),
            "order by did not start a line:\n{out}"
        );
        assert!(
            out.contains("left join `poc`.`orders`"),
            "the join qualifier was split from its JOIN:\n{out}"
        );
    }

    #[test]
    fn a_wide_parenthesis_gets_lines_of_its_own_and_a_narrow_one_does_not() {
        let out = laid_out(
            "select * from (t1 join t2 on t1.id = t2.id join t3 on t3.id = t2.id) where x = 1",
        );
        assert!(
            out.contains("from (\n"),
            "a wide FROM stayed inline:\n{out}"
        );
        assert!(
            out.contains("\n)"),
            "the wide span never closed its line:\n{out}"
        );

        let short = laid_out("select * from t where id in (1, 2, 3) and k = concat(a, b)");
        assert!(
            short.contains("in (1, 2, 3)"),
            "a short list was broken:\n{short}"
        );
        assert!(
            short.contains("concat(a, b)"),
            "a short call was broken:\n{short}"
        );
    }

    /// A `CREATE TABLE` normally arrives already laid out and never reaches
    /// this module. One with a long `enum` in it does, so what happens then is
    /// worth knowing rather than assuming.
    #[test]
    fn a_create_table_with_one_long_line_still_comes_out_as_sql() {
        let ddl = "CREATE TABLE `t` (\n  `id` int NOT NULL,\n  `state` enum('new','open','pending','on_hold','escalated','resolved','closed','archived','deleted') NOT NULL DEFAULT 'new',\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB";
        let out = tidy(ddl);
        assert_ne!(out, ddl, "the long line should have been laid out");
        assert_eq!(lex(&out), lex(ddl));
        assert!(
            out.contains("`id` int NOT NULL,"),
            "a column definition was mangled:\n{out}"
        );
        assert!(
            out.contains("PRIMARY KEY (`id`)"),
            "the key lost its columns:\n{out}"
        );
    }

    /// The one that decides whether this module runs at all.
    #[test]
    fn text_that_is_already_laid_out_is_returned_untouched() {
        let hand_written = "CREATE PROCEDURE `p`()\nBEGIN\n  SELECT 1;\nEND";
        assert_eq!(tidy(hand_written), hand_written);
        // Including text this module would have laid out differently: not
        // being worse than the author is the point.
        let odd = "SELECT 1,\n       2\n  FROM t";
        assert_eq!(tidy(odd), odd);
    }

    #[test]
    fn a_function_call_keeps_its_arguments_on_one_line() {
        let out =
            laid_out("select coalesce(sum(t.a), 0), left(t.name, 3), if(t.x > 1, 'y', 'n') from t");
        assert!(
            out.contains("coalesce(sum(t.a), 0)"),
            "an argument list was broken up:\n{out}"
        );
        // `left(` and `if(` are a join qualifier and a statement keyword by
        // spelling. Neither starts a line here.
        assert!(!out.contains("\nleft("), "LEFT( started a line:\n{out}");
        assert!(!out.contains("\nif("), "IF( started a line:\n{out}");
    }

    #[test]
    fn a_subquery_is_indented_and_a_value_list_is_not() {
        let out = laid_out(
            "select * from t where id in (select id from u where u.k = 1) and x in (1, 2, 3)",
        );
        assert!(out.contains("(\n"), "the subquery stayed inline:\n{out}");
        assert!(
            out.contains("in (1, 2, 3)"),
            "a value list was broken up:\n{out}"
        );
    }

    #[test]
    fn between_does_not_start_a_line_at_its_and() {
        let out = laid_out("select * from t where a between 1 and 9 and b = 2");
        assert!(out.contains("between 1 and 9"), "BETWEEN was split:\n{out}");
        // The *conjunction* still breaks — this is not "give up on AND".
        assert!(
            out.contains("\n  and b = 2"),
            "the conjunction did not break:\n{out}"
        );
    }

    #[test]
    fn a_routine_body_indents_its_blocks() {
        let body = ROUTINE_SAMPLE;
        assert_tokens_preserved(body);
        let out = laid_out(body);
        assert!(
            out.contains("\nBEGIN\n"),
            "BEGIN did not start a line:\n{out}"
        );
        assert!(
            out.contains("\n  DECLARE done"),
            "the body was not indented:\n{out}"
        );
        assert!(
            out.contains("\n  ELSE\n"),
            "ELSE did not line up with IF:\n{out}"
        );
        assert!(out.contains("\n  END IF;"), "END IF did not dedent:\n{out}");
        assert!(
            out.ends_with("END\n"),
            "the outer END did not dedent:\n{out}"
        );
    }

    /// `CASE` in a select list ends with `END` too, and that `END` closes an
    /// expression rather than a block. Getting this wrong dedents the whole
    /// rest of the statement.
    #[test]
    fn an_inline_case_does_not_dedent_what_follows() {
        let out =
            laid_out("select case when a = 1 then 'x' else 'y' end as k, b from t where c = 1");
        assert!(
            out.contains("end as k"),
            "the inline CASE broke its line:\n{out}"
        );
        assert!(out.contains("\nfrom t"), "FROM lost its line:\n{out}");
        assert!(out.contains("\nwhere c = 1"), "WHERE lost its line:\n{out}");
    }

    /// Nothing inside a quoted run or a comment is layout.
    #[test]
    fn strings_and_comments_are_carried_across_untouched() {
        let sql = "select 'a , b  from c', `weird ( name`, /* keep  this */ 1 -- trailing\nfrom t";
        assert_tokens_preserved(sql);
        let out = laid_out(sql);
        assert!(
            out.contains("'a , b  from c'"),
            "a string was re-spaced:\n{out}"
        );
        assert!(
            out.contains("/* keep  this */"),
            "a comment was re-spaced:\n{out}"
        );
    }

    #[test]
    fn a_number_with_an_exponent_survives_being_re_spaced() {
        assert_tokens_preserved("select 1.5e-3 + 2, x.y from t");
        let out = laid_out("select 1.5e-3 + 2 from t");
        assert!(out.contains("1.5e-3"), "the exponent was split:\n{out}");
    }

    /// An account name is not an expression: MySQL does not accept the spaces
    /// that re-spacing an operator would introduce.
    #[test]
    fn a_definer_keeps_its_at_sign_tight() {
        let out = laid_out("CREATE DEFINER=`root`@`localhost` VIEW `v` AS select 1");
        assert!(
            out.contains("`root`@`localhost`"),
            "the definer was split:\n{out}"
        );
    }

    /// Malformed input must not panic, and must not come back as something
    /// that looks fine.
    ///
    /// This asserts `tidy`'s contract rather than `layout`'s, because these are
    /// the inputs where the two differ: an unterminated string swallows the
    /// trailing newline the layout adds, the comparison sees it, and the
    /// original is handed back. That is the safety net doing its job, not a
    /// case to be excused.
    #[test]
    fn unterminated_quotes_and_comments_are_survivable() {
        for sql in [
            "select 'never closed from t",
            "select /* never closed",
            "select `never closed",
            "select ((( from",
            "select 'a' , /* fine */ 'b'",
            "",
        ] {
            let out = tidy(sql);
            assert!(
                out == sql || lex(&out) == lex(sql),
                "tidy changed the tokens of {sql:?}:\n{out}"
            );
        }
    }

    /// The safety net is not decoration: it must actually catch a layout that
    /// loses a token, whatever the rules above grow into.
    #[test]
    fn a_rewrite_that_changed_a_token_would_be_refused() {
        let wide = format!("select {} from t", "a, ".repeat(60));
        assert_ne!(tidy(&wide), wide, "a dense line should have been laid out");
        assert_eq!(lex(&tidy(&wide)), lex(&wide));
    }
}

#[cfg(test)]
mod show {
    /// Not an assertion — a way to read what this module actually produces.
    /// `cargo test --lib sqlfmt::show -- --nocapture --ignored`
    #[test]
    #[ignore]
    fn print_samples() {
        for sql in [super::tests::VIEW_SAMPLE, super::tests::ROUTINE_SAMPLE] {
            println!("{}\n----------------", super::tidy(sql));
        }
    }
}
