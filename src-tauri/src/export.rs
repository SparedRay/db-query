//! Getting a result set out of the app: CSV, `INSERT` scripts, clipboard text.
//!
//! # Where the rows come from
//!
//! The **grid** is the source of truth for "what is on screen", so exporting
//! what is shown means sending those rows back here rather than re-reading them
//! from the server. It costs one IPC hop over data that already made the trip
//! once, and it buys the only property that matters: what lands in the file is
//! what the user was looking at, including any column selection.
//!
//! # Two honest scopes, never a silent partial
//!
//! A result set in memory is already capped by `exec::MAX_ROWS`. Writing those
//! rows and calling it "the table" is the same class of failure as Stage 0's
//! silently-empty schema tree: no error, wrong answer. So [`ExportOutcome`]
//! carries the row count **and** whether the source was truncated, and the UI
//! is expected to say so.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::decode::{CellValue, ColumnMeta, TypeHint};
use crate::sqlgen;

/// A result set as the grid holds it.
///
/// One struct rather than three parallel parameters because they are one thing:
/// exporting `rows` without `truncated` is how a partial export gets presented
/// as a whole one, and exporting them without `columns` cannot decide quoting.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<CellValue>>,
    /// The row ceiling had already cut this short before it reached the grid.
    pub truncated: bool,
}

/// Both format's settings, carried together so a command does not need one
/// parameter per knob.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub csv: CsvOptions,
    pub inserts: InsertOptions,
}

// ------------------------------------------------------------------- outcome

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    pub path: String,
    pub rows_written: usize,
    /// The result set this came from had been cut short by the row ceiling.
    /// The export is complete for what was on screen and incomplete for the
    /// query — the UI must not present it as the latter.
    pub truncated_source: bool,
    pub bytes_written: u64,
    /// Things that are true and unwelcome: a binary column that cannot survive
    /// the trip, a NULL that CSV cannot distinguish from an empty string.
    pub warnings: Vec<String>,
}

// ----------------------------------------------------------------------- CSV

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvOptions {
    pub delimiter: String,
    /// RFC 4180 says CRLF, and Excel agrees. Off for Unix-tool-friendly output.
    pub crlf: bool,
    /// Without a BOM Excel mangles non-ASCII; with one some Unix tools show a
    /// stray `\u{feff}`. A real trade-off, so it is a choice rather than a default
    /// we impose silently.
    pub bom: bool,
    pub headers: bool,
    /// **CSV cannot distinguish NULL from an empty string.** They are different
    /// values, so which one collapses into the other is the user's call.
    pub null_as: String,
    /// A field starting `=`, `+`, `-` or `@` is executed as a formula by
    /// spreadsheets. Prefixing `'` defuses it — but it also *changes the
    /// exported value*, so it is off by default. A SQL client that quietly
    /// alters data is worse than one that warns.
    pub formula_guard: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: ",".into(),
            crlf: true,
            bom: true,
            headers: true,
            null_as: String::new(),
            formula_guard: false,
        }
    }
}

/// Render one CSV field, quoting only when the content requires it.
fn csv_field(raw: &str, opts: &CsvOptions) -> String {
    let guarded;
    let value = if opts.formula_guard && starts_like_a_formula(raw) {
        guarded = format!("'{raw}");
        &guarded
    } else {
        raw
    };

    let needs_quotes = value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
        || (!opts.delimiter.is_empty() && value.contains(&opts.delimiter));

    if needs_quotes {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn starts_like_a_formula(s: &str) -> bool {
    matches!(s.as_bytes().first(), Some(b'=' | b'+' | b'-' | b'@'))
}

/// The text a cell shows. NULL is the caller's policy; everything else is what
/// the grid displays, which is the whole point of exporting "what is shown".
fn cell_text(v: &CellValue, null_as: &str) -> String {
    match v {
        CellValue::Null => null_as.to_string(),
        CellValue::Int(i) => i.to_string(),
        CellValue::Float(f) => f.to_string(),
        CellValue::Bool(b) => if *b { "1" } else { "0" }.into(),
        CellValue::Text(t) => t.clone(),
        // What the grid shows, which is what "export what is shown" means. A
        // CSV of placeholders is honest; the SQL-INSERT export refuses instead,
        // because a placeholder written into a BLOB would be silent corruption.
        CellValue::Binary { bytes } => CellValue::binary_placeholder(*bytes),
    }
}

pub fn to_csv(columns: &[ColumnMeta], rows: &[Vec<CellValue>], opts: &CsvOptions) -> String {
    let nl = if opts.crlf { "\r\n" } else { "\n" };
    let mut out = String::new();
    if opts.bom {
        out.push('\u{feff}');
    }
    if opts.headers {
        let line: Vec<String> = columns.iter().map(|c| csv_field(&c.name, opts)).collect();
        out.push_str(&line.join(&opts.delimiter));
        out.push_str(nl);
    }
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .map(|c| csv_field(&cell_text(c, &opts.null_as), opts))
            .collect();
        out.push_str(&line.join(&opts.delimiter));
        out.push_str(nl);
    }
    out
}

// ------------------------------------------------------------------ INSERTs

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertOptions {
    /// Table name to write into. Defaults to the source table when there is
    /// one, but a result from a join has to be named by the user.
    pub table: String,
    pub db: Option<String>,
    pub create_table: bool,
    /// Rows per statement. Multi-row inserts are dramatically faster to replay
    /// and much smaller on disk than one statement per row.
    pub batch_size: usize,
}

/// Widen a protocol type name into something that is legal in a `CREATE TABLE`.
///
/// **The MySQL protocol does not carry length or precision.** `type_info()`
/// says `VARCHAR`, not `VARCHAR(190)`, so a `CREATE TABLE` derived from a result
/// set cannot reproduce the original column definitions — `VARCHAR` alone is a
/// syntax error. Each mapping below therefore widens to a type that can hold
/// anything the original could.
///
/// This is why [`crate::schema::table_ddl`] exists and is preferred: when the
/// rows came from a real table, the server's own `SHOW CREATE TABLE` is exact
/// and this guesswork is not used at all.
fn widened_ddl_type(sql_type: &str, hint: TypeHint) -> &'static str {
    match sql_type.to_ascii_uppercase().as_str() {
        "TINYINT" => "TINYINT",
        "SMALLINT" => "SMALLINT",
        "MEDIUMINT" => "MEDIUMINT",
        "INT" | "INTEGER" => "INT",
        "BIGINT" => "BIGINT",
        "TINYINT UNSIGNED" => "TINYINT UNSIGNED",
        "SMALLINT UNSIGNED" => "SMALLINT UNSIGNED",
        "MEDIUMINT UNSIGNED" => "MEDIUMINT UNSIGNED",
        "INT UNSIGNED" | "INTEGER UNSIGNED" => "INT UNSIGNED",
        "BIGINT UNSIGNED" => "BIGINT UNSIGNED",
        "FLOAT" => "FLOAT",
        "DOUBLE" => "DOUBLE",
        // The widest MySQL decimal. It cannot cover every DECIMAL — a
        // DECIMAL(65,0) has 65 integer digits and this allows 35 — but no single
        // type can, and this preserves the most scale.
        "DECIMAL" | "NEWDECIMAL" | "DECIMAL UNSIGNED" => "DECIMAL(65,30)",
        "BOOLEAN" | "BOOL" => "TINYINT(1)",
        "DATE" => "DATE",
        "TIME" => "TIME",
        "DATETIME" => "DATETIME(6)",
        "TIMESTAMP" => "TIMESTAMP(6) NULL",
        "YEAR" => "YEAR",
        "JSON" => "JSON",
        "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BINARY" | "VARBINARY" => "LONGBLOB",
        "GEOMETRY" => "GEOMETRY",
        _ => match hint {
            TypeHint::Numeric => "DECIMAL(65,30)",
            TypeHint::Temporal => "DATETIME(6)",
            TypeHint::Binary => "LONGBLOB",
            _ => "LONGTEXT",
        },
    }
}

/// A `CREATE TABLE` derived from a result set's column metadata.
///
/// Every column is nullable and there are no keys: the protocol carries neither.
/// The comment block says so, because a derived schema that *looks* like the
/// original is worse than one that admits what it is.
pub fn derived_create_table(
    columns: &[ColumnMeta],
    db: Option<&str>,
    table: &str,
) -> Result<String, String> {
    let target = match db {
        Some(d) => sqlgen::qualify(d, table)?,
        None => sqlgen::quote_ident(table)?,
    };
    let mut out = String::new();
    out.push_str(
        "-- Derived from the result set's column metadata, not from a real table.\n\
         -- The MySQL protocol carries no lengths, keys, defaults or nullability,\n\
         -- so types are widened to hold the data and every column is NULLable.\n\
         -- Review before using this as a schema.\n",
    );
    out.push_str(&format!("CREATE TABLE {target} (\n"));
    let cols: Vec<String> = columns
        .iter()
        .map(|c| {
            Ok(format!(
                "  {} {}",
                sqlgen::quote_ident(&c.name)?,
                widened_ddl_type(&c.sql_type, c.type_hint)
            ))
        })
        .collect::<Result<_, String>>()?;
    out.push_str(&cols.join(",\n"));
    out.push_str("\n);\n");
    Ok(out)
}

/// An `INSERT` script for a result set.
///
/// `create_table_sql` is passed in rather than derived here so the caller can
/// supply the server's exact `SHOW CREATE TABLE` when the rows came from a real
/// table, and fall back to [`derived_create_table`] only when they did not.
pub fn to_inserts(
    columns: &[ColumnMeta],
    rows: &[Vec<CellValue>],
    opts: &InsertOptions,
    create_table_sql: Option<&str>,
) -> Result<String, String> {
    let target = match &opts.db {
        Some(d) => sqlgen::qualify(d, &opts.table)?,
        None => sqlgen::quote_ident(&opts.table)?,
    };

    let mut out = String::new();
    if let Some(ddl) = create_table_sql {
        out.push_str(ddl);
        if !ddl.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }

    if rows.is_empty() {
        out.push_str("-- No rows to insert.\n");
        return Ok(out);
    }

    let col_list: Vec<String> = columns
        .iter()
        .map(|c| sqlgen::quote_ident(&c.name))
        .collect::<Result<_, String>>()?;
    let col_list = col_list.join(", ");

    let batch = opts.batch_size.max(1);
    for chunk in rows.chunks(batch) {
        out.push_str(&format!("INSERT INTO {target} ({col_list}) VALUES\n"));
        let mut tuples = Vec::with_capacity(chunk.len());
        for row in chunk {
            if row.len() != columns.len() {
                return Err(format!(
                    "A row has {} values but there are {} columns.",
                    row.len(),
                    columns.len()
                ));
            }
            let values: Vec<String> = row
                .iter()
                .zip(columns)
                .map(|(v, c)| sqlgen::literal(v, c.type_hint))
                .collect::<Result<_, String>>()?;
            tuples.push(format!("  ({})", values.join(", ")));
        }
        out.push_str(&tuples.join(",\n"));
        out.push_str(";\n");
    }
    Ok(out)
}

/// Columns whose values cannot survive an export, with the reason.
///
/// Called before writing so the outcome can carry a warning rather than the
/// file carrying a lie.
pub fn binary_column_warning(columns: &[ColumnMeta]) -> Option<String> {
    let names: Vec<&str> = columns
        .iter()
        .filter(|c| c.type_hint == TypeHint::Binary)
        .map(|c| c.name.as_str())
        .collect();
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "The binary column(s) {} hold only a size placeholder in the grid, not the \
         bytes — the export contains that placeholder. Use HEX({}) in your query to \
         export the real values.",
        names.join(", "),
        names[0],
    ))
}

// ------------------------------------------- streaming row rendering
//
// The buffered path above works from a decoded result set. The streaming path
// has one `MySqlRow` at a time and no result set at all, so it renders straight
// from the row — using the same `decode` and `sqlgen` calls, so the two paths
// cannot disagree about what a value looks like.

use crate::decode::{columns_of, decode_cell};
use sqlx::mysql::MySqlRow;
use sqlx::Row as _;

fn row_cells(row: &MySqlRow) -> Vec<CellValue> {
    (0..row.len()).map(|i| decode_cell(row, i)).collect()
}

pub fn csv_header(row: &MySqlRow, opts: &CsvOptions) -> String {
    if !opts.headers {
        return String::new();
    }
    let nl = if opts.crlf { "\r\n" } else { "\n" };
    let mut out = String::new();
    if opts.bom {
        out.push('\u{feff}');
    }
    let names: Vec<String> = columns_of(row)
        .iter()
        .map(|c| csv_field(&c.name, opts))
        .collect();
    out.push_str(&names.join(&opts.delimiter));
    out.push_str(nl);
    out
}

pub fn csv_row(row: &MySqlRow, opts: &CsvOptions) -> String {
    let nl = if opts.crlf { "\r\n" } else { "\n" };
    let fields: Vec<String> = row_cells(row)
        .iter()
        .map(|c| csv_field(&cell_text(c, &opts.null_as), opts))
        .collect();
    format!("{}{nl}", fields.join(&opts.delimiter))
}

pub fn insert_header(row: &MySqlRow, opts: &InsertOptions) -> Result<String, String> {
    if !opts.create_table {
        return Ok(String::new());
    }
    // Streaming has only result metadata to work from, so the schema is the
    // derived, widened one — and it says so in the file.
    derived_create_table(&columns_of(row), opts.db.as_deref(), &opts.table)
        .map(|d| format!("{d}\n"))
}

/// One `INSERT` per row on the streaming path.
///
/// Deliberately not batched: batching needs rows held back until a chunk is
/// full, and the entire point of this path is that nothing is held. One
/// statement per row is larger on disk and replays fine.
pub fn insert_row(row: &MySqlRow, opts: &InsertOptions, buf: &mut String) -> Result<(), String> {
    let columns = columns_of(row);
    let target = match &opts.db {
        Some(d) => sqlgen::qualify(d, &opts.table)?,
        None => sqlgen::quote_ident(&opts.table)?,
    };
    let names: Vec<String> = columns
        .iter()
        .map(|c| sqlgen::quote_ident(&c.name))
        .collect::<Result<_, String>>()?;
    let values: Vec<String> = row_cells(row)
        .iter()
        .zip(&columns)
        .map(|(v, c)| sqlgen::literal(v, c.type_hint))
        .collect::<Result<_, String>>()?;
    buf.push_str(&format!(
        "INSERT INTO {target} ({}) VALUES ({});\n",
        names.join(", "),
        values.join(", ")
    ));
    Ok(())
}

// ------------------------------------------------------- unbounded re-run

/// What an unbounded re-run should write.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Csv,
    Inserts,
}

/// Is this statement safe to re-run on its own, outside the script it came from?
///
/// The rule is deliberately narrow. A statement is re-runnable when it is **one
/// row-returning statement and nothing else**. Anything else risks either
/// running a modification a second time or producing different rows: a script
/// whose earlier statements set up temp tables or moved the current database
/// will not reproduce standalone, and re-running an `INSERT` to "export" it
/// would be a data-corrupting surprise.
///
/// When we cannot promise it, the unbounded path is not offered at all — which
/// is better than offering it and being wrong once.
pub fn rerunnable(sql: &str) -> Result<String, String> {
    let split_out = crate::split::split(sql);
    if split_out.delimiter_detected {
        return Err("A script with a DELIMITER block cannot be re-run for export.".into());
    }
    match split_out.statements.as_slice() {
        [one] => {
            let text = &sql[one.start..one.end];
            let kind = crate::exec::classify(text);
            if crate::exec::returns_rows(kind) {
                Ok(text.to_string())
            } else {
                Err("Only statements that return rows can be re-run for export.".into())
            }
        }
        [] => Err("There is no statement to re-run.".into()),
        _ => Err(
            "This result came from a multi-statement script, so re-running it on its own \
             could produce different rows. Export the rows that are shown instead."
                .into(),
        ),
    }
}

// -------------------------------------------------------------------- writing

pub fn write(path: &Path, body: &str) -> Result<u64, String> {
    let bytes = body.as_bytes();
    crate::files::write_atomic(path, bytes)?;
    Ok(bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, sql_type: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            type_hint: crate::decode::type_hint(sql_type),
            sql_type: sql_type.into(),
        }
    }

    fn txt(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }

    /// A minimal RFC 4180 reader, written independently of the writer so a
    /// shared misunderstanding of the format cannot make a round trip pass.
    fn parse_csv(input: &str, delim: char) -> Vec<Vec<String>> {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                } else {
                    field.push(c);
                }
            } else if c == '"' && field.is_empty() {
                in_quotes = true;
            } else if c == delim {
                row.push(std::mem::take(&mut field));
            } else if c == '\r' {
                // swallow; the \n that follows ends the record
            } else if c == '\n' {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            } else {
                field.push(c);
            }
        }
        if !field.is_empty() || !row.is_empty() {
            row.push(field);
            rows.push(row);
        }
        rows
    }

    // -------------------------------------------------------- the wire

    /// The exact shapes `src/api.ts` sends. A rename on either side must fail
    /// here rather than the first time someone clicks Export.
    #[test]
    fn export_inputs_deserialise_from_what_the_frontend_sends() {
        let rs: ResultSet = serde_json::from_str(
            r#"{"columns":[{"name":"id","typeHint":"numeric","sqlType":"INT"}],
                "rows":[[1],[null],["x"],[true],[1.5]],
                "truncated":true}"#,
        )
        .expect("ResultSet");
        assert!(rs.truncated);
        assert_eq!(rs.columns[0].name, "id");
        // Untagged order matters: `1` must stay an Int, not become a Float.
        assert_eq!(rs.rows[0][0], CellValue::Int(1));
        assert_eq!(rs.rows[1][0], CellValue::Null);
        assert_eq!(rs.rows[2][0], CellValue::Text("x".into()));
        assert_eq!(rs.rows[3][0], CellValue::Bool(true));
        assert_eq!(rs.rows[4][0], CellValue::Float(1.5));

        let csv: CsvOptions = serde_json::from_str(
            r#"{"delimiter":",","crlf":true,"bom":true,"headers":true,
                "nullAs":"","formulaGuard":false}"#,
        )
        .expect("CsvOptions");
        assert_eq!(csv.delimiter, ",");

        let ins: InsertOptions = serde_json::from_str(
            r#"{"table":"users","db":"poc","createTable":true,"batchSize":100}"#,
        )
        .expect("InsertOptions");
        assert_eq!(ins.batch_size, 100);
        assert_eq!(ins.db.as_deref(), Some("poc"));

        for (json, want) in [
            (r#""csv""#, ExportFormat::Csv),
            (r#""inserts""#, ExportFormat::Inserts),
        ] {
            assert_eq!(serde_json::from_str::<ExportFormat>(json).unwrap(), want);
        }
    }

    // ---------------------------------------------------------------- CSV

    /// **E5.** Every character that forces quoting, in one table, round-tripped
    /// through an independent reader.
    #[test]
    fn hostile_values_survive_a_csv_round_trip() {
        let columns = vec![col("id", "INT"), col("v", "VARCHAR")];
        let hostile = vec![
            "plain",
            "has,comma",
            "has\"quote",
            "has\nnewline",
            "has\r\ncrlf",
            "  leading and trailing  ",
            "héllo ★ 表 🔐",
            "",
            "a,b\"c\nd",
        ];
        let rows: Vec<Vec<CellValue>> = hostile
            .iter()
            .enumerate()
            .map(|(i, s)| vec![CellValue::Int(i as i64), txt(s)])
            .collect();

        let opts = CsvOptions::default();
        let csv = to_csv(&columns, &rows, &opts);
        let parsed = parse_csv(&csv, ',');

        assert_eq!(parsed[0], vec!["id", "v"], "header row");
        for (i, expected) in hostile.iter().enumerate() {
            let got = &parsed[i + 1][1];
            // Exact equality, including a CRLF *inside* a quoted field: RFC 4180
            // preserves the bytes between the quotes, so anything less than
            // byte-identical here is data loss.
            assert_eq!(got, expected, "row {i} changed: {expected:?} -> {got:?}");
        }
    }

    /// The one thing CSV genuinely cannot express. Which value wins is a
    /// decision, so it is a setting, and both directions are tested.
    #[test]
    fn null_and_empty_string_are_distinguishable_only_by_policy() {
        let columns = vec![col("v", "VARCHAR")];
        let rows = vec![vec![CellValue::Null], vec![txt("")]];

        let default = to_csv(&columns, &rows, &CsvOptions::default());
        let parsed = parse_csv(&default, ',');
        assert_eq!(parsed[1][0], "", "NULL becomes an empty field by default");
        assert_eq!(
            parsed[2][0], "",
            "and so does an empty string — hence the warning"
        );

        let explicit = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                null_as: "NULL".into(),
                ..CsvOptions::default()
            },
        );
        let parsed = parse_csv(&explicit, ',');
        assert_eq!(parsed[1][0], "NULL");
        assert_eq!(
            parsed[2][0], "",
            "an empty string must not become the word NULL"
        );
    }

    #[test]
    fn quoting_happens_only_where_it_is_needed() {
        let columns = vec![col("a", "VARCHAR"), col("b", "VARCHAR")];
        let rows = vec![vec![txt("plain"), txt("needs,quotes")]];
        let csv = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                bom: false,
                ..Default::default()
            },
        );
        assert!(csv.contains("plain,\"needs,quotes\""), "{csv:?}");
    }

    #[test]
    fn crlf_and_bom_are_choices() {
        let columns = vec![col("a", "INT")];
        let rows = vec![vec![CellValue::Int(1)]];

        let excel = to_csv(&columns, &rows, &CsvOptions::default());
        assert!(excel.starts_with('\u{feff}'), "BOM missing");
        assert!(excel.contains("\r\n"), "CRLF missing");

        let unix = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                bom: false,
                crlf: false,
                ..Default::default()
            },
        );
        assert!(!unix.starts_with('\u{feff}'));
        assert!(!unix.contains('\r'));
    }

    /// A field starting `=` is executed by spreadsheets. The guard defuses it —
    /// and because it *changes the value*, it is off unless asked for.
    #[test]
    fn the_formula_guard_is_opt_in_and_visible() {
        let columns = vec![col("v", "VARCHAR")];
        let rows = vec![
            vec![txt("=1+1")],
            vec![txt("-5")],
            vec![txt("@x")],
            vec![txt("+7")],
        ];

        let unguarded = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                bom: false,
                ..Default::default()
            },
        );
        assert!(unguarded.contains("=1+1"), "must not alter data by default");
        assert!(!unguarded.contains("'=1+1"));

        let guarded = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                bom: false,
                formula_guard: true,
                ..Default::default()
            },
        );
        for s in ["'=1+1", "'-5", "'@x", "'+7"] {
            assert!(guarded.contains(s), "{s} not guarded in {guarded:?}");
        }
    }

    #[test]
    fn headers_can_be_omitted() {
        let columns = vec![col("a", "INT")];
        let rows = vec![vec![CellValue::Int(1)]];
        let csv = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                headers: false,
                bom: false,
                ..Default::default()
            },
        );
        assert!(!csv.contains('a'), "{csv:?}");
    }

    #[test]
    fn an_alternative_delimiter_drives_quoting_too() {
        let columns = vec![col("a", "VARCHAR"), col("b", "VARCHAR")];
        let rows = vec![vec![txt("x;y"), txt("plain,text")]];
        let csv = to_csv(
            &columns,
            &rows,
            &CsvOptions {
                delimiter: ";".into(),
                bom: false,
                ..Default::default()
            },
        );
        assert!(csv.contains("\"x;y\";plain,text"), "{csv:?}");
    }

    // ------------------------------------------------------------ INSERTs

    fn insert_opts() -> InsertOptions {
        InsertOptions {
            table: "users".into(),
            db: Some("poc".into()),
            create_table: false,
            batch_size: 2,
        }
    }

    #[test]
    fn inserts_are_batched_and_quoted() {
        let columns = vec![col("id", "INT"), col("email", "VARCHAR")];
        let rows = vec![
            vec![CellValue::Int(1), txt("ada@example.com")],
            vec![CellValue::Int(2), txt("it's@example.com")],
            vec![CellValue::Int(3), CellValue::Null],
        ];
        let out = to_inserts(&columns, &rows, &insert_opts(), None).unwrap();

        assert_eq!(
            out.matches("INSERT INTO").count(),
            2,
            "batch_size 2 over 3 rows:\n{out}"
        );
        assert!(
            out.contains("INSERT INTO `poc`.`users` (`id`, `email`) VALUES"),
            "{out}"
        );
        assert!(out.contains(r"(2, 'it\'s@example.com')"), "{out}");
        assert!(out.contains("(3, NULL)"), "NULL must not be quoted:\n{out}");
    }

    /// The bug the whole stage was most at risk of shipping.
    #[test]
    fn big_integers_and_decimals_are_not_quoted_in_inserts() {
        let columns = vec![col("big", "BIGINT"), col("money", "DECIMAL")];
        let rows = vec![vec![txt("9223372036854775807"), txt("12345678901234.99")]];
        let out = to_inserts(&columns, &rows, &insert_opts(), None).unwrap();
        assert!(
            out.contains("(9223372036854775807, 12345678901234.99)"),
            "{out}"
        );
        assert!(!out.contains('\''), "numeric values were quoted:\n{out}");
    }

    /// The bytes are gone by the time a cell exists, so an INSERT would write
    /// `<binary, 12 bytes>` into the column. Refusing is the only honest answer.
    #[test]
    fn a_binary_column_stops_the_insert_export() {
        let columns = vec![col("id", "INT"), col("data", "BLOB")];
        let rows = vec![vec![
            CellValue::Int(1),
            CellValue::Binary { bytes: Some(12) },
        ]];
        let err = to_inserts(&columns, &rows, &insert_opts(), None).expect_err("must refuse");
        assert!(err.to_lowercase().contains("binary"), "{err}");
        assert!(binary_column_warning(&columns).is_some());
    }

    /// The other half, and the bug: a binary-*collation* `VARCHAR` reports a
    /// BLOB type name, so the column hint says binary — but the value decoded
    /// to text and the grid shows it as text. The INSERT export used to refuse
    /// it anyway.
    #[test]
    fn text_in_a_binary_typed_column_still_exports() {
        let columns = vec![col("id", "INT"), col("name", "VARBINARY")];
        let rows = vec![vec![CellValue::Int(1), txt("ada")]];
        let out = to_inserts(&columns, &rows, &insert_opts(), None)
            .expect("text that the grid shows must be exportable");
        assert!(out.contains("'ada'"), "{out}");
    }

    /// A CSV of what is shown is honest, so binary does not stop it — only the
    /// INSERT export, where a placeholder would become silent corruption.
    #[test]
    fn a_binary_value_still_renders_in_csv() {
        let columns = vec![col("data", "BLOB")];
        let rows = vec![vec![CellValue::Binary { bytes: Some(12) }]];
        let out = to_csv(&columns, &rows, &CsvOptions::default());
        assert!(out.contains("<binary, 12 bytes>"), "{out}");
    }

    #[test]
    fn no_rows_produces_a_comment_not_a_broken_statement() {
        let columns = vec![col("id", "INT")];
        let out = to_inserts(&columns, &[], &insert_opts(), None).unwrap();
        assert!(!out.contains("INSERT INTO"), "{out}");
        assert!(out.contains("No rows"), "{out}");
    }

    #[test]
    fn a_supplied_create_table_is_placed_before_the_inserts() {
        let columns = vec![col("id", "INT")];
        let rows = vec![vec![CellValue::Int(1)]];
        let ddl = "CREATE TABLE `poc`.`users` (`id` int);\n";
        let out = to_inserts(&columns, &rows, &insert_opts(), Some(ddl)).unwrap();
        assert!(
            out.find("CREATE TABLE").unwrap() < out.find("INSERT INTO").unwrap(),
            "{out}"
        );
    }

    /// `VARCHAR` alone is a syntax error — the protocol carries no length — so
    /// a derived schema must widen rather than echo the type name.
    #[test]
    fn a_derived_create_table_widens_types_and_says_so() {
        let columns = vec![
            col("id", "BIGINT"),
            col("name", "VARCHAR"),
            col("price", "DECIMAL"),
            col("blob", "BLOB"),
        ];
        let out = derived_create_table(&columns, Some("poc"), "t").unwrap();
        assert!(out.contains("CREATE TABLE `poc`.`t` ("), "{out}");
        assert!(out.contains("`id` BIGINT"), "{out}");
        assert!(
            out.contains("`name` LONGTEXT"),
            "VARCHAR needs a length: {out}"
        );
        assert!(out.contains("`price` DECIMAL(65,30)"), "{out}");
        assert!(out.contains("`blob` LONGBLOB"), "{out}");
        assert!(
            out.contains("Derived from the result set"),
            "must admit what it is:\n{out}"
        );
        assert!(
            !out.contains("VARCHAR,"),
            "a bare VARCHAR would not parse: {out}"
        );
    }

    // ------------------------------------------------------ re-runnability

    /// The unbounded path re-executes a statement on its own. Offering it for
    /// something that cannot reproduce — or worse, that modifies data — is how
    /// an "export" becomes a second `DELETE`.
    #[test]
    fn only_a_single_row_returning_statement_is_rerunnable() {
        assert_eq!(
            rerunnable("SELECT * FROM users").unwrap(),
            "SELECT * FROM users"
        );
        assert!(rerunnable("SHOW TABLES").is_ok(), "SHOW returns rows");

        assert!(
            rerunnable("DELETE FROM users").is_err(),
            "must never re-run a DELETE"
        );
        assert!(rerunnable("UPDATE users SET a = 1").is_err());
        assert!(rerunnable("INSERT INTO t VALUES (1)").is_err());
        assert!(rerunnable("").is_err());
        assert!(
            rerunnable("USE poc; SELECT * FROM users").is_err(),
            "a multi-statement script may not reproduce standalone"
        );
        assert!(
            rerunnable("DELIMITER $$\nSELECT 1$$").is_err(),
            "a routine block cannot be re-run for export"
        );
    }

    #[test]
    fn a_row_of_the_wrong_width_is_an_error_not_a_broken_script() {
        let columns = vec![col("a", "INT"), col("b", "INT")];
        let rows = vec![vec![CellValue::Int(1)]];
        assert!(to_inserts(&columns, &rows, &insert_opts(), None).is_err());
    }
}
