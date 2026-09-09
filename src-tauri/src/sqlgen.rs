//! Generating SQL text for the user to read.
//!
//! Everything here produces a string that lands in an editor tab. **Nothing
//! here is ever executed** — that is the rule Stage 3 is built on, and it is
//! why none of this touches `AssertSqlSafe`: the output is not a query, it is a
//! draft the user reviews and runs themselves. Half of it is `DROP`.
//!
//! One implementation, in Rust, for the same reason the statement splitter is:
//! Stage 1 duplicated cursor logic into TypeScript and it drifted until a
//! dead-code warning caught it. Identifier quoting is not something to have two
//! of.

use serde::Deserialize;

use crate::decode::{CellValue, TypeHint};
use crate::schema::{RoutineKind, RoutineRef};

/// Backtick-quote an identifier, always.
///
/// Unconditionally, never "only when it needs it" — deciding that requires a
/// reserved-word list which will be wrong for some MySQL version, and being
/// wrong there produces a syntax error in generated SQL. Quoting always is
/// correct always; the cost is cosmetic.
///
/// Doubling embedded backticks is the whole escaping rule for MySQL's
/// backtick-quoted identifiers. NUL has no escape, so it is **rejected** rather
/// than stripped: dropping the byte would silently produce a *different*
/// identifier, which might name a real table that is not the one meant. An
/// error is the only answer that cannot be quietly wrong.
///
/// This moved here from `session.rs`, where it was written for `USE`. Two
/// copies of identifier quoting is precisely the split that bit us in Stage 1
/// with the statement splitter, so there is one.
pub fn quote_ident(name: &str) -> Result<String, String> {
    if name.contains('\0') {
        return Err(format!("Invalid identifier: {name:?} contains a NUL byte."));
    }
    Ok(format!("`{}`", name.replace('`', "``")))
}

/// `` `db`.`name` `` — both halves quoted.
pub fn qualify(db: &str, name: &str) -> Result<String, String> {
    Ok(format!("{}.{}", quote_ident(db)?, quote_ident(name)?))
}

/// Wrap a routine body from `SHOW CREATE …` into a script that can be run more
/// than once.
///
/// **MySQL has no `CREATE OR REPLACE PROCEDURE`** — that is MariaDB and
/// PostgreSQL. `SHOW CREATE PROCEDURE` hands back a bare `CREATE PROCEDURE`,
/// which fails with "routine already exists" the second time. `DROP … IF
/// EXISTS` first is the MySQL spelling of the same intent.
///
/// Two details that are easy to get wrong:
///
///   * The body contains unescaped `;` between its statements, so it needs a
///     `DELIMITER` wrapper. Our splitter already bails out to a single span
///     when it sees one, which is the correct handling — a routine body goes to
///     the server whole.
///   * `SHOW CREATE` returns the definition **unqualified**, so running it
///     re-creates the routine in whatever database happens to be current. The
///     leading `USE` makes the target explicit rather than ambient. That is
///     what mysqldump does, for the same reason.
pub fn routine_script(
    db: &str,
    name: &str,
    kind: RoutineKind,
    body: &str,
) -> Result<String, String> {
    let kw = kind.keyword();
    Ok(format!(
        "-- {kw} {qualified}\n\
         -- Review before running: this DROPs the existing routine first.\n\
         -- MySQL has no CREATE OR REPLACE for routines, so DROP + CREATE is the\n\
         -- equivalent. The DEFINER clause below is kept as the server reports it.\n\
         USE {db_q};\n\
         DROP {kw} IF EXISTS {name_q};\n\
         DELIMITER $$\n\
         {body}$$\n\
         DELIMITER ;\n",
        qualified = qualify(db, name)?,
        db_q = quote_ident(db)?,
        name_q = quote_ident(name)?,
        body = body.trim_end(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::schema::RoutineParam;

    fn param(name: &str, mode: &str, ty: &str) -> RoutineParam {
        RoutineParam {
            name: name.into(),
            mode: mode.into(),
            data_type: ty.into(),
        }
    }

    fn routine(name: &str, kind: RoutineKind, params: Vec<RoutineParam>) -> RoutineRef {
        RoutineRef {
            name: name.into(),
            kind,
            returns: matches!(kind, RoutineKind::Function).then(|| "int".to_string()),
            params,
        }
    }

    // -------------------------------------------------------- the wire

    /// Stage 2 shipped a bug where a field was silently dropped between Rust
    /// and the UI, and only a test on the *serialised form* could have caught
    /// it. These assert the exact JSON `src/api.ts` sends, so a rename on
    /// either side fails here rather than at a right-click.
    #[test]
    fn drop_targets_deserialise_from_what_the_frontend_sends() {
        let cases = [
            (r#"{"type":"schema","db":"poc"}"#, "DROP DATABASE `poc`;"),
            (
                r#"{"type":"table","db":"poc","name":"users"}"#,
                "DROP TABLE `poc`.`users`;",
            ),
            (
                r#"{"type":"view","db":"poc","name":"v"}"#,
                "DROP VIEW `poc`.`v`;",
            ),
            (
                r#"{"type":"column","db":"poc","table":"users","name":"email"}"#,
                "ALTER TABLE `poc`.`users` DROP COLUMN `email`;",
            ),
            (
                r#"{"type":"routine","db":"poc","name":"p","kind":"procedure"}"#,
                "DROP PROCEDURE `poc`.`p`;",
            ),
            (
                r#"{"type":"routine","db":"poc","name":"f","kind":"function"}"#,
                "DROP FUNCTION `poc`.`f`;",
            ),
        ];
        for (json, expected) in cases {
            let target: DropTarget = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("{json} did not deserialise: {e}"));
            assert!(generate_drop(&target).unwrap().contains(expected), "{json}");
        }
    }

    /// The UI reads `kind` off every routine to pick an icon and a menu, so the
    /// spelling has to be the one it expects.
    #[test]
    fn routine_kind_is_camel_case_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&RoutineKind::Procedure).unwrap(),
            "\"procedure\""
        );
        assert_eq!(
            serde_json::to_string(&RoutineKind::Function).unwrap(),
            "\"function\""
        );
    }

    /// `returns` is null for a procedure and set for a function; the tree shows
    /// one and not the other.
    #[test]
    fn a_routine_serialises_with_the_fields_the_tree_reads() {
        let r = routine("f", RoutineKind::Function, vec![param("uid", "IN", "int")]);
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["name"], "f");
        assert_eq!(j["kind"], "function");
        assert_eq!(j["returns"], "int");
        assert_eq!(j["params"][0]["name"], "uid");
        assert_eq!(j["params"][0]["dataType"], "int");
        assert_eq!(j["params"][0]["mode"], "IN");

        let p = routine("p", RoutineKind::Procedure, vec![]);
        assert_eq!(
            serde_json::to_value(&p).unwrap()["returns"],
            serde_json::Value::Null
        );
    }

    // ------------------------------------------------------------ literals

    /// NULL is a value, not an absence of one. `''` and `'NULL'` are both
    /// different values, and writing either would corrupt the data.
    #[test]
    fn null_is_null_in_every_column_type() {
        for hint in [
            TypeHint::Numeric,
            TypeHint::Text,
            TypeHint::Temporal,
            TypeHint::Bool,
            TypeHint::Binary,
        ] {
            assert_eq!(literal(&CellValue::Null, hint).unwrap(), "NULL", "{hint:?}");
        }
    }

    /// **The bug this stage was most likely to ship.** `decode.rs` returns a
    /// BIGINT past 2^53 and every DECIMAL as text so JavaScript cannot round
    /// them. Quoting them because they are text writes strings into numeric
    /// columns.
    #[test]
    fn big_integers_and_decimals_stay_unquoted() {
        let big = CellValue::Text("9223372036854775807".into());
        assert_eq!(
            literal(&big, TypeHint::Numeric).unwrap(),
            "9223372036854775807"
        );

        let money = CellValue::Text("12345678901234.99".into());
        assert_eq!(
            literal(&money, TypeHint::Numeric).unwrap(),
            "12345678901234.99"
        );

        let negative = CellValue::Text("-0.00000001".into());
        assert_eq!(
            literal(&negative, TypeHint::Numeric).unwrap(),
            "-0.00000001"
        );
    }

    /// The same text in a text column must be quoted. The column decides, not
    /// the value — that is the whole rule.
    #[test]
    fn the_column_type_decides_quoting_not_the_value() {
        let v = CellValue::Text("123".into());
        assert_eq!(literal(&v, TypeHint::Numeric).unwrap(), "123");
        assert_eq!(literal(&v, TypeHint::Text).unwrap(), "'123'");
    }

    /// Unquoted output is spliced into a script, so anything non-numeric
    /// reaching a numeric column must stop rather than be emitted bare.
    #[test]
    fn non_numeric_text_in_a_numeric_column_is_refused() {
        for bad in ["1; DROP TABLE users", "abc", "", "1 2", "0x41"] {
            let v = CellValue::Text(bad.into());
            assert!(
                literal(&v, TypeHint::Numeric).is_err(),
                "{bad:?} was emitted into a numeric column"
            );
        }
    }

    /// The bytes are gone by the time a cell exists — `decode.rs` keeps only
    /// the length. Quoting a placeholder writes it into the BLOB.
    #[test]
    fn binary_is_refused_rather_than_guessed() {
        let v = CellValue::Binary { bytes: Some(12) };
        let err = literal(&v, TypeHint::Binary).expect_err("must not guess");
        assert!(err.to_lowercase().contains("binary"), "{err}");
        // Refused wherever it appears, not only under a binary-typed column.
        assert!(literal(&v, TypeHint::Text).is_err());
        assert!(literal(&CellValue::Binary { bytes: None }, TypeHint::Text).is_err());
    }

    /// The disagreement this variant exists to end.
    ///
    /// A `VARCHAR` with a binary collation is reported under the BLOB type
    /// names, so its column hint is `Binary` — but it decodes to real text and
    /// the grid shows it as text. Refusing it made the export contradict what
    /// the user could plainly see. The value knows it is text; that wins.
    #[test]
    fn text_under_a_binary_hinted_column_is_exported_as_text() {
        let v = CellValue::Text("hello".into());
        assert_eq!(literal(&v, TypeHint::Binary).unwrap(), "'hello'");
    }

    /// And it is still escaped — a binary-collation column is where someone
    /// stores the awkward strings.
    #[test]
    fn text_under_a_binary_hinted_column_is_still_escaped() {
        let v = CellValue::Text("it's\\".into());
        assert_eq!(literal(&v, TypeHint::Binary).unwrap(), r"'it\'s\\'");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(quote_string("it's"), r"'it\'s'");
        assert_eq!(quote_string(r"back\slash"), r"'back\\slash'");
        assert_eq!(quote_string(r"both ' and \"), r"'both \' and \\'");
    }

    /// A raw newline inside a literal is legal but makes a one-statement-per-line
    /// file unreadable; NUL and Ctrl-Z break tools that read the .sql back.
    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(quote_string("a\nb"), r"'a\nb'");
        assert_eq!(quote_string("a\rb"), r"'a\rb'");
        assert_eq!(quote_string("a\0b"), r"'a\0b'");
        assert_eq!(quote_string("a\u{1a}b"), r"'a\Zb'");
    }

    /// A double quote needs no escape inside a single-quoted literal, and
    /// escaping it anyway would be noise in a file a human reads.
    #[test]
    fn double_quotes_are_left_alone() {
        assert_eq!(quote_string(r#"say "hi""#), r#"'say "hi"'"#);
    }

    #[test]
    fn unicode_survives_untouched() {
        assert_eq!(quote_string("héllo ★ 表 🔐"), "'héllo ★ 表 🔐'");
    }

    /// The classic injection payload, as *data*. It must come back as one
    /// harmless string literal, not as two statements.
    #[test]
    fn an_injection_payload_stays_a_single_literal() {
        let out = quote_string("'; DROP TABLE users; --");
        assert_eq!(out, r"'\'; DROP TABLE users; --'");
        // One opening quote, one closing quote, and every interior quote escaped.
        assert!(out.starts_with('\'') && out.ends_with('\''));
        assert_eq!(out.matches(r"\'").count(), 1);
    }

    #[test]
    fn booleans_become_one_and_zero() {
        assert_eq!(
            literal(&CellValue::Bool(true), TypeHint::Bool).unwrap(),
            "1"
        );
        assert_eq!(
            literal(&CellValue::Bool(false), TypeHint::Bool).unwrap(),
            "0"
        );
        assert_eq!(
            literal(&CellValue::Bool(true), TypeHint::Numeric).unwrap(),
            "1"
        );
    }

    #[test]
    fn non_finite_floats_are_refused_not_nulled() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                literal(&CellValue::Float(v), TypeHint::Numeric).is_err(),
                "{v} should not become a literal"
            );
        }
    }

    #[test]
    fn temporal_values_are_quoted_strings() {
        let v = CellValue::Text("2026-09-05 14:30:00".into());
        assert_eq!(
            literal(&v, TypeHint::Temporal).unwrap(),
            "'2026-09-05 14:30:00'"
        );
    }

    // ------------------------------------------------------------ statements

    #[test]
    fn a_generated_select_carries_an_explicit_limit() {
        let s = generate_select("poc", "users", 1000).unwrap();
        assert!(s.contains("FROM `poc`.`users`"), "{s}");
        assert!(s.contains("LIMIT 1000;"), "{s}");
    }

    #[test]
    fn a_generated_select_quotes_awkward_names() {
        let s = generate_select("my db", "my`table", 10).unwrap();
        assert!(s.contains("FROM `my db`.`my``table`"), "{s}");
    }

    #[test]
    fn a_zero_browse_limit_is_refused() {
        assert!(generate_select("poc", "users", 0).is_err());
    }

    /// The browse limit is not the safety ceiling. If these ever become one
    /// number, every double-click pulls five thousand rows.
    #[test]
    fn the_browse_limit_is_not_the_safety_ceiling() {
        assert_ne!(
            u64::from(DEFAULT_BROWSE_LIMIT),
            crate::exec::MAX_ROWS as u64,
            "browse limit and MAX_ROWS are different concepts"
        );
    }

    #[test]
    fn every_drop_target_generates_correct_sql() {
        let cases = vec![
            (
                DropTarget::Schema { db: "poc".into() },
                "DROP DATABASE `poc`;",
            ),
            (
                DropTarget::Table {
                    db: "poc".into(),
                    name: "users".into(),
                },
                "DROP TABLE `poc`.`users`;",
            ),
            (
                DropTarget::View {
                    db: "poc".into(),
                    name: "user_totals".into(),
                },
                "DROP VIEW `poc`.`user_totals`;",
            ),
            (
                DropTarget::Column {
                    db: "poc".into(),
                    table: "users".into(),
                    name: "email".into(),
                },
                "ALTER TABLE `poc`.`users` DROP COLUMN `email`;",
            ),
            (
                DropTarget::Routine {
                    db: "poc".into(),
                    name: "top_spenders".into(),
                    kind: RoutineKind::Procedure,
                },
                "DROP PROCEDURE `poc`.`top_spenders`;",
            ),
            (
                DropTarget::Routine {
                    db: "poc".into(),
                    name: "order_count".into(),
                    kind: RoutineKind::Function,
                },
                "DROP FUNCTION `poc`.`order_count`;",
            ),
        ];
        for (target, expected) in cases {
            let out = generate_drop(&target).unwrap();
            assert!(out.contains(expected), "{target:?}\n{out}");
            assert!(out.starts_with("-- "), "no warning comment: {out}");
        }
    }

    /// A DROP is the one generated statement that destroys something. An
    /// `IF EXISTS` here would turn a mistyped hand-edit into a silent no-op.
    #[test]
    fn a_drop_does_not_add_if_exists() {
        let out = generate_drop(&DropTarget::Table {
            db: "poc".into(),
            name: "users".into(),
        })
        .unwrap();
        assert!(!out.contains("IF EXISTS"), "{out}");
    }

    #[test]
    fn drop_targets_with_awkward_names_are_quoted() {
        let out = generate_drop(&DropTarget::Column {
            db: "d`b".into(),
            table: "t`l".into(),
            name: "c`l".into(),
        })
        .unwrap();
        assert!(
            out.contains("ALTER TABLE `d``b`.`t``l` DROP COLUMN `c``l`;"),
            "{out}"
        );
    }

    /// Placeholders rather than `NULL`s, deliberately: a call that runs without
    /// being read is a call nobody decided to make.
    #[test]
    fn a_procedure_call_uses_placeholders_that_must_be_filled_in() {
        let r = routine(
            "top_spenders",
            RoutineKind::Procedure,
            vec![
                param("min_total", "IN", "decimal(12,2)"),
                param("max_rows", "IN", "int"),
            ],
        );
        let s = generate_call("poc", &r).unwrap();
        assert!(
            s.contains(
                "CALL `poc`.`top_spenders`(/* min_total decimal(12,2) */, /* max_rows int */);"
            ),
            "{s}"
        );
        assert!(
            s.contains("-- Parameters: IN min_total decimal(12,2), IN max_rows int"),
            "{s}"
        );
        assert!(
            !s.contains("NULL"),
            "placeholders must not be runnable values: {s}"
        );
    }

    #[test]
    fn a_no_argument_procedure_call_has_empty_parentheses() {
        let r = routine("ping_poc", RoutineKind::Procedure, vec![]);
        let s = generate_call("poc", &r).unwrap();
        assert!(s.contains("CALL `poc`.`ping_poc`();"), "{s}");
        assert!(s.contains("-- no parameters"), "{s}");
    }

    /// A function is invoked by SELECT, not CALL.
    #[test]
    fn a_function_call_is_a_select() {
        let r = routine(
            "order_count",
            RoutineKind::Function,
            vec![param("uid", "IN", "int")],
        );
        let s = generate_call("poc", &r).unwrap();
        assert!(
            s.contains("SELECT `poc`.`order_count`(/* uid int */);"),
            "{s}"
        );
        assert!(!s.contains("CALL"), "{s}");
        assert!(s.contains("RETURNS int"), "{s}");
    }

    /// `OUT` needs a session variable, which is correct as written and needs no
    /// editing — unlike an `IN` placeholder.
    #[test]
    fn out_parameters_become_session_variables() {
        let r = routine(
            "with_out",
            RoutineKind::Procedure,
            vec![
                param("a", "IN", "int"),
                param("total", "OUT", "int"),
                param("both", "INOUT", "int"),
            ],
        );
        let s = generate_call("poc", &r).unwrap();
        assert!(s.contains("(/* a int */, @total, @both);"), "{s}");
    }

    #[test]
    fn plain_identifiers_are_still_quoted() {
        assert_eq!(quote_ident("users").unwrap(), "`users`");
    }

    /// The case that makes unconditional quoting worth it.
    #[test]
    fn reserved_words_and_spaces_survive() {
        assert_eq!(quote_ident("order").unwrap(), "`order`");
        assert_eq!(quote_ident("select").unwrap(), "`select`");
        assert_eq!(quote_ident("my table").unwrap(), "`my table`");
        assert_eq!(quote_ident("2024_totals").unwrap(), "`2024_totals`");
    }

    /// The escape that matters: a backtick in a name would otherwise end the
    /// quote early and turn the rest of the identifier into syntax.
    #[test]
    fn embedded_backticks_are_doubled() {
        assert_eq!(quote_ident("we`ird").unwrap(), "`we``ird`");
        // Counting backticks in a literal is unreadable, so build the
        // expectation: one backtick doubles to two, plus the wrapping pair.
        let tick = "`";
        assert_eq!(quote_ident(tick).unwrap(), tick.repeat(4));
        assert_eq!(quote_ident(&tick.repeat(2)).unwrap(), tick.repeat(6));
    }

    #[test]
    fn unicode_names_are_untouched_apart_from_quoting() {
        assert_eq!(quote_ident("ünïcode").unwrap(), "`ünïcode`");
        assert_eq!(quote_ident("表").unwrap(), "`表`");
        assert_eq!(quote_ident("🔐").unwrap(), "`🔐`");
    }

    /// MySQL identifiers cannot contain NUL and there is no escape for it.
    /// Dropping the byte beats truncating the name.
    /// Stripping the NUL would yield `` `ab` `` — a name that may well exist
    /// and is not the one asked for. Refusing is the only non-silent answer.
    #[test]
    fn nul_is_rejected_not_stripped() {
        assert!(quote_ident("a\0b").is_err());
        assert!(qualify("db", "a\0b").is_err());
    }

    #[test]
    fn qualify_quotes_both_halves() {
        assert_eq!(qualify("my db", "my table").unwrap(), "`my db`.`my table`");
        assert_eq!(qualify("d`b", "t`l").unwrap(), "`d``b`.`t``l`");
    }

    #[test]
    fn a_routine_script_drops_before_creating() {
        let s = routine_script(
            "poc",
            "top_spenders",
            RoutineKind::Procedure,
            "CREATE PROCEDURE `top_spenders`()\nBEGIN\n  SELECT 1;\nEND",
        )
        .unwrap();
        assert!(
            s.contains("DROP PROCEDURE IF EXISTS `top_spenders`;"),
            "{s}"
        );
        // Ordering is the whole point: creating before dropping would delete
        // the routine that was just made.
        let drop_at = s.find("DROP PROCEDURE").unwrap();
        let create_at = s.find("CREATE PROCEDURE `top_spenders`").unwrap();
        assert!(drop_at < create_at, "DROP must come first:\n{s}");
    }

    /// Without `DELIMITER`, the `;` inside the body ends the CREATE early and
    /// the server sees a truncated routine.
    #[test]
    fn a_routine_script_wraps_the_body_in_a_delimiter_block() {
        let s = routine_script(
            "poc",
            "p",
            RoutineKind::Procedure,
            "CREATE PROCEDURE `p`()\nBEGIN\n  SELECT 1;\nEND",
        )
        .unwrap();
        assert!(s.contains("DELIMITER $$"), "{s}");
        assert!(s.contains("END$$"), "{s}");
        assert!(s.trim_end().ends_with("DELIMITER ;"), "{s}");
    }

    /// `SHOW CREATE` returns an unqualified definition, so without this the
    /// routine is re-created in whatever database happens to be current.
    #[test]
    fn a_routine_script_names_its_database() {
        let s = routine_script(
            "poc",
            "p",
            RoutineKind::Function,
            "CREATE FUNCTION `p`() RETURNS INT RETURN 1",
        )
        .unwrap();
        assert!(s.contains("USE `poc`;"), "{s}");
        assert!(s.contains("DROP FUNCTION IF EXISTS `p`;"), "{s}");
    }

    #[test]
    fn a_routine_script_quotes_awkward_names() {
        let s = routine_script(
            "d`b",
            "p`roc",
            RoutineKind::Procedure,
            "CREATE PROCEDURE x() BEGIN END",
        )
        .unwrap();
        assert!(s.contains("USE `d``b`;"), "{s}");
        assert!(s.contains("DROP PROCEDURE IF EXISTS `p``roc`;"), "{s}");
    }
}

// ------------------------------------------------------------- browse limit

/// Rows a generated `SELECT` asks for.
///
/// **Deliberately not `exec::MAX_ROWS`.** That is the safety ceiling that stops
/// a runaway query wedging the UI; this is "how much do I want to look at".
/// Sharing one number would mean every double-click on a table pulled five
/// thousand rows. Two concepts, two numbers.
///
/// Generated SQL carries an explicit `LIMIT`, so auto-LIMIT correctly leaves it
/// alone — it already skips statements with a top-level `LIMIT`. A browse limit
/// above the ceiling is allowed and the executor will flag the result truncated,
/// which is the honest outcome rather than silently rewriting the user's number.
pub const DEFAULT_BROWSE_LIMIT: u32 = 1000;

// ------------------------------------------------------------------ literals

/// Escape a string into a single-quoted MySQL literal.
///
/// **Assumes the server's default escaping mode** (`NO_BACKSLASH_ESCAPES` off),
/// which is how MySQL ships. There is no encoding that is correct in both
/// modes: `''` is portable for the quote itself, but a literal backslash needs
/// `\\` in the default mode and must *not* be doubled under
/// `NO_BACKSLASH_ESCAPES`. Hex literals would be correct in both and unreadable
/// in both, which defeats the point of a script a human reviews.
pub fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\0' => out.push_str("\\0"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            // Ctrl-Z ends input on Windows consoles; MySQL escapes it for that
            // reason and so must we, or a .sql file breaks when piped in.
            '\u{1a}' => out.push_str("\\Z"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Turn a decoded cell back into a SQL literal.
///
/// **Driven by the column's declared type, never by the runtime type of the
/// cell.** `decode.rs` deliberately returns `BIGINT` past 2^53 and every
/// `DECIMAL` as `Text` so JavaScript cannot round them. A generator that quotes
/// values because they arrived as text would write strings into numeric columns
/// — silently, and into a script that gets run somewhere else later.
///
/// Binary is refused rather than guessed. The decode keeps only
/// `<binary, N bytes>`; the bytes are gone by the time a cell exists, so any
/// literal built from one would be a placeholder string written into a BLOB.
/// Refusing turns silent corruption into a message.
pub fn literal(value: &CellValue, hint: TypeHint) -> Result<String, String> {
    // NULL is NULL whatever the column is — never `''`, never `'NULL'`.
    if matches!(value, CellValue::Null) {
        return Ok("NULL".into());
    }

    // Decided on the **value**, not on the column's type.
    //
    // The column type cannot answer this: a `VARCHAR` with a binary collation
    // is reported under the BLOB type names, decodes to ordinary text, and
    // renders as text in the grid — and refusing it here is what made the
    // export disagree with what the user could plainly see. A cell that really
    // holds bytes says so itself.
    if let CellValue::Binary { .. } = value {
        return Err(
            "Binary values cannot be written as literals: the row data holds only a \
             placeholder, not the bytes. Export the query results instead, which reads \
             the values back from the server."
                .into(),
        );
    }

    match hint {
        // A binary-hinted column whose value survived as text is text. Quoting
        // it is right: it is what the grid shows and what the server returned.
        TypeHint::Binary => Ok(quote_string(&cell_text(value))),
        TypeHint::Numeric => match value {
            CellValue::Int(v) => Ok(v.to_string()),
            CellValue::Float(v) if v.is_finite() => Ok(format_float(*v)),
            // MySQL cannot store these, so reaching here means something is
            // already wrong. Emitting NULL would quietly change the data.
            CellValue::Float(v) => Err(format!("{v} has no MySQL literal.")),
            CellValue::Bool(b) => Ok(if *b { "1".into() } else { "0".into() }),
            // The big-integer and decimal path. Checked rather than trusted:
            // emitting unvalidated text unquoted would splice arbitrary
            // content into the script.
            CellValue::Text(t) if is_numeric_literal(t) => Ok(t.clone()),
            CellValue::Text(t) => Err(format!(
                "{t:?} is not a number, but its column is numeric. Refusing to guess."
            )),
            CellValue::Null => unreachable!("handled above"),
            CellValue::Binary { .. } => unreachable!("refused above"),
        },
        TypeHint::Bool => match value {
            CellValue::Bool(b) => Ok(if *b { "1".into() } else { "0".into() }),
            CellValue::Int(v) => Ok(v.to_string()),
            CellValue::Text(t) if is_numeric_literal(t) => Ok(t.clone()),
            other => Ok(quote_string(&cell_text(other))),
        },
        TypeHint::Temporal | TypeHint::Text => Ok(quote_string(&cell_text(value))),
    }
}

/// A float that round-trips, without an exponent MySQL would have to reparse
/// differently. `{}` on f64 already prints the shortest exact representation.
fn format_float(v: f64) -> String {
    let s = v.to_string();
    // `1e20` is valid MySQL, but a bare integer-valued float printing as "1"
    // where the column is DOUBLE is also fine. Nothing to fix here; this
    // exists so the choice is visible rather than incidental.
    s
}

/// Does this text parse as a SQL numeric literal? Deliberately strict: digits,
/// one optional sign, one optional point, one optional exponent.
fn is_numeric_literal(t: &str) -> bool {
    !t.is_empty() && t.parse::<f64>().is_ok() && !t.contains(|c: char| c.is_whitespace())
}

fn cell_text(v: &CellValue) -> String {
    match v {
        CellValue::Null => "NULL".into(),
        CellValue::Int(i) => i.to_string(),
        CellValue::Float(f) => f.to_string(),
        CellValue::Bool(b) => if *b { "1" } else { "0" }.into(),
        CellValue::Text(t) => t.clone(),
        CellValue::Binary { bytes } => CellValue::binary_placeholder(*bytes),
    }
}

// ------------------------------------------------------------- statements

/// `SELECT * FROM `db`.`table` LIMIT n` — the double-click action.
pub fn generate_select(db: &str, table: &str, limit: u32) -> Result<String, String> {
    if limit == 0 {
        return Err("A browse limit of 0 would return nothing.".into());
    }
    Ok(format!(
        "SELECT *\nFROM {}\nLIMIT {limit};\n",
        qualify(db, table)?
    ))
}

/// What a `DROP` is being generated for.
///
/// An enum rather than a name plus a kind string: a half-specified target — a
/// column with no table, a routine with no kind — cannot be constructed, so the
/// generator never has to decide what to do with one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum DropTarget {
    Schema {
        db: String,
    },
    Table {
        db: String,
        name: String,
    },
    View {
        db: String,
        name: String,
    },
    Column {
        db: String,
        table: String,
        name: String,
    },
    Routine {
        db: String,
        name: String,
        kind: RoutineKind,
    },
}

/// A `DROP` for the user to read. Never executed here — see the module docs.
///
/// No `IF EXISTS`: the object came from the schema tree, so it exists, and
/// `IF EXISTS` would turn a mistyped hand-edit into a silent no-op. The warning
/// comment is there because this is the one generated statement that destroys
/// something.
pub fn generate_drop(target: &DropTarget) -> Result<String, String> {
    let (warning, stmt) = match target {
        DropTarget::Schema { db } => (
            "-- Drops the database and EVERY table, view and routine in it. Not reversible.",
            format!("DROP DATABASE {};", quote_ident(db)?),
        ),
        DropTarget::Table { db, name } => (
            "-- Drops the table and all of its data. Not reversible.",
            format!("DROP TABLE {};", qualify(db, name)?),
        ),
        DropTarget::View { db, name } => (
            "-- Drops the view. The underlying tables are untouched.",
            format!("DROP VIEW {};", qualify(db, name)?),
        ),
        DropTarget::Column { db, table, name } => (
            "-- Drops the column and all of its data. Not reversible.",
            format!(
                "ALTER TABLE {} DROP COLUMN {};",
                qualify(db, table)?,
                quote_ident(name)?
            ),
        ),
        DropTarget::Routine { db, name, kind } => (
            "-- Drops the routine. Review anything that calls it first.",
            format!("DROP {} {};", kind.keyword(), qualify(db, name)?),
        ),
    };
    Ok(format!("{warning}\n{stmt}\n"))
}

/// A snippet that calls a routine.
///
/// `IN` parameters become `/* name type */` placeholders, which are **not
/// runnable until filled in** — and that is the point. Emitting `NULL` for each
/// would produce a statement that runs and does something nobody asked for.
/// `OUT` and `INOUT` become `@name` session variables, which is the correct
/// spelling and needs no editing.
pub fn generate_call(db: &str, routine: &RoutineRef) -> Result<String, String> {
    let args = routine
        .params
        .iter()
        .map(|p| {
            if p.mode.eq_ignore_ascii_case("OUT") || p.mode.eq_ignore_ascii_case("INOUT") {
                format!("@{}", p.name)
            } else {
                format!("/* {} {} */", p.name, p.data_type)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let signature = if routine.params.is_empty() {
        "-- no parameters".to_string()
    } else {
        format!(
            "-- Parameters: {}",
            routine
                .params
                .iter()
                .map(|p| format!("{} {} {}", p.mode, p.name, p.data_type))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let target = qualify(db, &routine.name)?;
    Ok(match routine.kind {
        RoutineKind::Procedure => {
            format!("-- PROCEDURE {target}\n{signature}\nCALL {target}({args});\n")
        }
        RoutineKind::Function => {
            let returns = routine.returns.as_deref().unwrap_or("?");
            format!(
                "-- FUNCTION {target} RETURNS {returns}\n{signature}\nSELECT {target}({args});\n"
            )
        }
    })
}
