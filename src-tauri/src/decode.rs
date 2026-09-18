//! Row decoding: sqlx MySQL values -> typed JSON the grid can render.
//!
//! Rules that matter:
//!   * NULL becomes JSON null, never the string "NULL", so the grid can style
//!     it distinctly from a column that literally contains "NULL".
//!   * DECIMAL stays Text. Routing money through f64 corrupts it silently.
//!   * Integers outside +-2^53 become Text. JS numbers cannot hold them, so
//!     emitting Int would quietly mangle large IDs in the grid.

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// Largest integer JavaScript can represent exactly.
const JS_SAFE_INT: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TypeHint {
    Numeric,
    Text,
    Temporal,
    Binary,
    Bool,
}

// `Deserialize` so a result set can come back from the UI to be exported. The
// grid is the source of truth for "what is on screen", and export must match
// what the user is looking at.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    pub type_hint: TypeHint,
    /// Raw MySQL type name, shown on hover — useful when a decode looks wrong.
    pub sql_type: String,
}

/// Untagged in both directions. Deserialization tries the variants in order,
/// which is why `Int` precedes `Float`: `1` must not become `1.0`.
///
/// # Why `Binary` is a variant and not a string
///
/// The bytes of a BLOB never reach a cell — the grid would only show a
/// placeholder anyway — so what a cell holds is the *fact* that they were
/// binary, plus how many there were. That fact used to be carried by the
/// placeholder **text** `"<binary, 12 bytes>"`, which made a real string of
/// those characters indistinguishable from a real BLOB.
///
/// Worse, nothing downstream could rely on it, so `sqlgen::literal` decided
/// from the *column type* instead — and a `VARCHAR` with a binary collation is
/// reported under the BLOB type names. Such a column decodes to real text and
/// renders as text, and the SQL-INSERT export refused it as binary. Carrying
/// the truth on the value is what lets both agree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellValue {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    /// Bytes that are not text. `None` when even the length is unknown, which
    /// is what a failed `Vec<u8>` decode leaves us with.
    Binary {
        bytes: Option<usize>,
    },
}

impl CellValue {
    /// How a binary cell reads in the grid, and in anything that renders a row
    /// as text. One definition, so the two cannot disagree about the wording.
    pub fn binary_placeholder(bytes: Option<usize>) -> String {
        match bytes {
            Some(n) => format!("<binary, {n} bytes>"),
            None => "<binary>".to_string(),
        }
    }
}

impl CellValue {
    fn null() -> Self {
        CellValue::Null
    }
}

pub fn type_hint(sql_type: &str) -> TypeHint {
    let t = sql_type.to_ascii_uppercase();
    match t.as_str() {
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT"
        | "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "INTEGER UNSIGNED" | "BIGINT UNSIGNED" | "FLOAT" | "DOUBLE" | "DECIMAL"
        | "NEWDECIMAL" | "FLOAT UNSIGNED" | "DOUBLE UNSIGNED" | "DECIMAL UNSIGNED" => {
            TypeHint::Numeric
        }
        // Elasticsearch's own numeric names. The hint drives alignment and
        // nothing else, so it maps type *names* rather than one engine's types
        // — a second engine that says "long" should still right-align.
        "LONG" | "SHORT" | "BYTE" | "HALF_FLOAT" | "SCALED_FLOAT" | "UNSIGNED_LONG" => {
            TypeHint::Numeric
        }
        "BOOLEAN" | "BOOL" => TypeHint::Bool,
        "DATE" | "TIME" | "DATETIME" | "TIMESTAMP" | "YEAR" => TypeHint::Temporal,
        "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BINARY" | "VARBINARY" | "GEOMETRY" => {
            TypeHint::Binary
        }
        _ => TypeHint::Text,
    }
}

pub fn columns_of(row: &MySqlRow) -> Vec<ColumnMeta> {
    row.columns()
        .iter()
        .map(|c| {
            let sql_type = c.type_info().name().to_string();
            ColumnMeta {
                name: c.name().to_string(),
                type_hint: type_hint(&sql_type),
                sql_type,
            }
        })
        .collect()
}

/// Decode one cell. Never panics: an unhandled or malformed type degrades to
/// Text, or to a placeholder, rather than taking the query down.
pub fn decode_cell(row: &MySqlRow, idx: usize) -> CellValue {
    let raw = match row.try_get_raw(idx) {
        Ok(r) => r,
        Err(_) => return CellValue::Text("<unreadable>".into()),
    };
    let sql_type = raw.type_info().name().to_ascii_uppercase();

    if raw.is_null() {
        // **sqlx calls a zero date NULL**, on purpose: in the binary protocol it
        // is the single byte 0, and `sqlx-mysql-0.9.0/src/value.rs:102` says
        // "zero dates and date times should be treated the same as NULL".
        // MySQL does not, and nor does the grid, which shows
        // `0000-00-00 00:00:00` from the text protocol — so an export wrote
        // NULL where the grid had a value. A real NULL has no bytes at all, so
        // reading the bytes tells the two apart.
        if matches!(sql_type.as_str(), "DATE" | "DATETIME" | "TIMESTAMP") {
            if let Some(zero) = raw_bytes(row, idx).and_then(|b| binary_temporal(&b, &sql_type)) {
                return CellValue::Text(zero);
            }
        }
        return CellValue::null();
    }

    match sql_type.as_str() {
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" => {
            match row.try_get::<i64, _>(idx) {
                Ok(v) if v.abs() <= JS_SAFE_INT => CellValue::Int(v),
                Ok(v) => CellValue::Text(v.to_string()),
                Err(_) => text_fallback(row, idx),
            }
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "INTEGER UNSIGNED" | "BIGINT UNSIGNED" => match row.try_get::<u64, _>(idx) {
            Ok(v) if v <= JS_SAFE_INT as u64 => CellValue::Int(v as i64),
            Ok(v) => CellValue::Text(v.to_string()),
            Err(_) => text_fallback(row, idx),
        },
        "BOOLEAN" | "BOOL" => match row.try_get::<bool, _>(idx) {
            Ok(v) => CellValue::Bool(v),
            Err(_) => text_fallback(row, idx),
        },
        "FLOAT" | "FLOAT UNSIGNED" => match row.try_get::<f32, _>(idx) {
            Ok(v) => CellValue::Float(v as f64),
            Err(_) => text_fallback(row, idx),
        },
        "DOUBLE" | "DOUBLE UNSIGNED" => match row.try_get::<f64, _>(idx) {
            Ok(v) => CellValue::Float(v),
            Err(_) => text_fallback(row, idx),
        },
        // DECIMAL deliberately stays textual — see module docs.
        "DECIMAL" | "NEWDECIMAL" | "DECIMAL UNSIGNED" => decimal_text(row, idx),
        "DATE" | "TIME" | "DATETIME" | "TIMESTAMP" | "YEAR" => temporal_text(row, idx, &sql_type),
        "JSON" => text_fallback(row, idx),
        // MySQL BIT is an unsigned integer of N bits. Rendering the raw bytes
        // as text gives replacement characters; BIT(1) as a boolean flag is
        // common enough (every Hibernate schema) that "170" beats "\u{fffd}".
        "BIT" => match raw_bytes(row, idx) {
            Some(b) if !b.is_empty() && b.len() <= 8 => {
                CellValue::Int(b.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64) as i64)
            }
            _ => text_fallback(row, idx),
        },
        "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BINARY" | "VARBINARY" | "GEOMETRY" => {
            match row.try_get::<Vec<u8>, _>(idx) {
                // Text first: a string column with a *binary collation* is
                // reported under exactly these type names. See `bytes_as_text`.
                Ok(b) => match bytes_as_text(&b) {
                    Some(s) => CellValue::Text(s),
                    None => CellValue::Binary {
                        bytes: Some(b.len()),
                    },
                },
                Err(_) => CellValue::Binary { bytes: None },
            }
        }
        _ => text_fallback(row, idx),
    }
}

fn decimal_text(row: &MySqlRow, idx: usize) -> CellValue {
    if let Ok(s) = row.try_get::<String, _>(idx) {
        return CellValue::Text(s);
    }
    if let Ok(d) = row.try_get::<sqlx::types::BigDecimal, _>(idx) {
        return CellValue::Text(d.to_string());
    }
    text_fallback(row, idx)
}

/// A date or time, as text, whichever protocol it arrived in.
///
/// **Two protocols, and only one of them was handled.** The grid runs queries
/// through the text protocol, where the server has already formatted the value.
/// Export streams through a prepared statement, the **binary** protocol, where a
/// `DATETIME` is a length byte and packed integers. sqlx's `NaiveDateTime`
/// accepts only a column typed exactly `DATETIME` — read from
/// `sqlx-mysql-0.9.0/src/types/chrono.rs:206` on 2026-09-18: it has no
/// `compatible()` override, unlike `DateTime<Utc>` — so on a **`TIMESTAMP`**
/// column it refused, every typed attempt after it refused too, and the last
/// resort wrote the packed bytes out as lossy UTF-8. The grid looked right only
/// because, in the text protocol, those raw bytes *are* the formatted value.
/// The export of a real `client_calculation` query came out as
/// `\x07\u{fffd}\x07\x09\x0b\x02-5` for `2026-09-11 02:45:53`.
///
/// So the binary form is decoded here, by the column's own type, from MySQL's
/// documented layout. That also covers what sqlx's types cannot hold at all: a
/// zero date, a `TIME` past 24 hours or below zero, and `YEAR`.
fn temporal_text(row: &MySqlRow, idx: usize, sql_type: &str) -> CellValue {
    if let Some(b) = raw_bytes(row, idx) {
        if !is_server_text(&b, sql_type) {
            return CellValue::Text(
                binary_temporal(&b, sql_type).unwrap_or_else(|| "<undecodable>".into()),
            );
        }
    }
    text_temporal(row, idx)
}

/// Did the server format this value, or pack it?
///
/// Formatted temporal text is printable ASCII — digits, `-`, `:`, `.` and a
/// space. The packed form of a date or time starts with its length, a byte
/// below 0x20. `YEAR` has no length byte (it is a plain 2-byte integer), so it
/// is text exactly when it is digits.
fn is_server_text(b: &[u8], sql_type: &str) -> bool {
    if sql_type == "YEAR" {
        return !b.is_empty() && b.iter().all(u8::is_ascii_digit);
    }
    !b.is_empty() && b.iter().all(|c| c.is_ascii_graphic() || *c == b' ')
}

/// MySQL's binary-protocol temporal layouts. `None` for a shape that is none
/// of them, so the caller says "undecodable" rather than printing bytes.
///
///   * `DATE` / `DATETIME` / `TIMESTAMP`: length (0, 4, 7 or 11), then year
///     (u16 LE), month, day, [hour, minute, second, [microseconds u32 LE]].
///     Length 0 is the zero date.
///   * `TIME`: length (0, 8 or 12), then negative flag, days (u32 LE), hour,
///     minute, second, [microseconds u32 LE].
///   * `YEAR`: a u16 LE, no length byte.
pub fn binary_temporal(b: &[u8], sql_type: &str) -> Option<String> {
    let u32_at = |p: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?))
    };

    if sql_type == "YEAR" {
        let v = u16::from_le_bytes(b.get(..2)?.try_into().ok()?);
        return (b.len() == 2).then(|| format!("{v:04}"));
    }

    let len = *b.first()? as usize;
    let p = b.get(1..1 + len)?;
    if b.len() != 1 + len {
        return None;
    }

    if sql_type == "TIME" {
        if len == 0 {
            return Some("00:00:00".into());
        }
        if len != 8 && len != 12 {
            return None;
        }
        let micros = if len == 12 { u32_at(p, 8)? } else { 0 };
        let hours = u64::from(u32_at(p, 1)?) * 24 + u64::from(p[5]);
        return Some(format!(
            "{}{hours:02}:{:02}:{:02}{}",
            if p[0] == 1 { "-" } else { "" },
            p[6],
            p[7],
            fraction(micros)
        ));
    }

    let (y, mo, d) = match len {
        0 => (0, 0, 0),
        4 | 7 | 11 => (u16::from_le_bytes([p[0], p[1]]), p[2], p[3]),
        _ => return None,
    };
    let date = format!("{y:04}-{mo:02}-{d:02}");
    if sql_type == "DATE" {
        return Some(date);
    }
    let (h, mi, s) = if len >= 7 {
        (p[4], p[5], p[6])
    } else {
        (0, 0, 0)
    };
    let micros = if len == 11 { u32_at(p, 7)? } else { 0 };
    Some(format!("{date} {h:02}:{mi:02}:{s:02}{}", fraction(micros)))
}

/// Fractional seconds as chrono's `%.f` prints them — nothing, three digits or
/// six — so a value reads the same in the grid and in the export.
fn fraction(micros: u32) -> String {
    match micros {
        0 => String::new(),
        m if m % 1000 == 0 => format!(".{:03}", m / 1000),
        m => format!(".{m:06}"),
    }
}

/// The text protocol, unchanged: what the grid has always shown.
fn text_temporal(row: &MySqlRow, idx: usize) -> CellValue {
    use sqlx::types::chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    if let Ok(v) = row.try_get::<NaiveDateTime, _>(idx) {
        return CellValue::Text(v.format("%Y-%m-%d %H:%M:%S%.f").to_string());
    }
    if let Ok(v) = row.try_get::<NaiveDate, _>(idx) {
        return CellValue::Text(v.to_string());
    }
    if let Ok(v) = row.try_get::<NaiveTime, _>(idx) {
        return CellValue::Text(v.to_string());
    }
    text_fallback(row, idx)
}

/// The bytes behind a cell, whatever sqlx thinks its type is.
///
/// `try_get_unchecked` skips the **type-compatibility check** that `try_get`
/// performs before it decodes anything. That check is what makes a readable
/// JSON or BIT column come back as an error rather than as its contents.
fn raw_bytes(row: &MySqlRow, idx: usize) -> Option<Vec<u8>> {
    row.try_get_unchecked::<Vec<u8>, _>(idx).ok()
}

/// Are these bytes really text, or really binary?
///
/// The type name cannot answer it. MySQL reports a string column with a
/// **binary collation** — `utf8mb4_bin`, and every `... BINARY` column — under
/// the same names as a BLOB, so `VARBINARY` covers both a password hash and a
/// perfectly readable case-sensitive name. The *bytes* can answer it: valid
/// UTF-8 with no control characters is text by any reasonable reading, and
/// showing it beats printing `<binary, 16 bytes>` over legible words.
fn bytes_as_text(b: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(b).ok()?;
    // A NUL or a stray control byte means a blob that happens to decode, not
    // text. Tab, newline and carriage return are ordinary text.
    if s.chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return None;
    }
    Some(s.to_owned())
}

/// Last resort: string, then raw bytes as lossy UTF-8, then a placeholder.
fn text_fallback(row: &MySqlRow, idx: usize) -> CellValue {
    if let Ok(s) = row.try_get::<String, _>(idx) {
        return CellValue::Text(s);
    }
    if let Ok(b) = row.try_get::<Vec<u8>, _>(idx) {
        return CellValue::Text(String::from_utf8_lossy(&b).into_owned());
    }
    // The one that actually rescues JSON. sqlx's typed accessors check type
    // *compatibility* and bail before decoding anything, so both attempts above
    // fail on a value that is perfectly readable — a JSON column came out as
    // `<undecodable>` for exactly this reason. Raw bytes face no such gate.
    if let Some(b) = raw_bytes(row, idx) {
        return CellValue::Text(String::from_utf8_lossy(&b).into_owned());
    }
    CellValue::Text("<undecodable>".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------ binary temporals

    /// **The report, byte for byte.** A `TIMESTAMP` from the real export:
    /// `07 EA 07 09 0B 02 2D 35` is 2026-09-11 02:45:53, and `EA` is not valid
    /// UTF-8 — which is where the `\u{fffd}` in the CSV came from.
    #[test]
    fn the_exported_timestamp_decodes() {
        let b = [0x07, 0xEA, 0x07, 0x09, 0x0B, 0x02, 0x2D, 0x35];
        assert!(!is_server_text(&b, "TIMESTAMP"));
        assert_eq!(
            binary_temporal(&b, "TIMESTAMP").as_deref(),
            Some("2026-09-11 02:45:53")
        );
        assert_eq!(
            binary_temporal(&b, "DATETIME").as_deref(),
            Some("2026-09-11 02:45:53")
        );
    }

    #[test]
    fn midnight_microseconds_and_dates_decode() {
        // Length 4: a DATETIME at exactly midnight sends no time at all.
        let midnight = [0x04, 0xEA, 0x07, 0x01, 0x02];
        assert_eq!(
            binary_temporal(&midnight, "DATETIME").as_deref(),
            Some("2026-01-02 00:00:00")
        );
        assert_eq!(
            binary_temporal(&midnight, "DATE").as_deref(),
            Some("2026-01-02")
        );

        // Length 11: 120 000 µs prints as chrono's `%.f` would, `.120`.
        let mut ms = vec![0x0B, 0xEA, 0x07, 0x09, 0x0B, 0x02, 0x2D, 0x35];
        ms.extend_from_slice(&120_000u32.to_le_bytes());
        assert_eq!(
            binary_temporal(&ms, "DATETIME").as_deref(),
            Some("2026-09-11 02:45:53.120")
        );
        let mut us = ms[..8].to_vec();
        us.extend_from_slice(&123_456u32.to_le_bytes());
        assert_eq!(
            binary_temporal(&us, "TIMESTAMP").as_deref(),
            Some("2026-09-11 02:45:53.123456")
        );
    }

    /// What sqlx's types cannot hold at all.
    #[test]
    fn zero_dates_long_times_and_years_decode() {
        assert_eq!(
            binary_temporal(&[0x00], "DATETIME").as_deref(),
            Some("0000-00-00 00:00:00")
        );
        assert_eq!(
            binary_temporal(&[0x00], "DATE").as_deref(),
            Some("0000-00-00")
        );
        assert_eq!(
            binary_temporal(&[0x00], "TIME").as_deref(),
            Some("00:00:00")
        );

        // -838:59:59, the bottom of TIME's range: negative, 34 days, 22 hours.
        let t = [0x08, 0x01, 34, 0, 0, 0, 22, 59, 59];
        assert_eq!(binary_temporal(&t, "TIME").as_deref(), Some("-838:59:59"));

        let y = 2026u16.to_le_bytes();
        assert!(!is_server_text(&y, "YEAR"));
        assert_eq!(binary_temporal(&y, "YEAR").as_deref(), Some("2026"));
    }

    /// The text protocol is left alone, so the grid does not change.
    #[test]
    fn server_formatted_text_is_recognised_as_text() {
        assert!(is_server_text(b"2026-09-11 02:45:53", "TIMESTAMP"));
        assert!(is_server_text(b"-838:59:59", "TIME"));
        assert!(is_server_text(b"2026", "YEAR"));
        assert!(is_server_text(b"0000-00-00", "DATE"));
    }

    /// A shape that is none of the layouts says so instead of printing bytes.
    #[test]
    fn a_malformed_packed_value_is_undecodable_not_garbage() {
        assert_eq!(binary_temporal(&[0x07, 0xEA], "DATETIME"), None);
        assert_eq!(binary_temporal(&[0x05, 1, 2, 3, 4, 5], "DATETIME"), None);
        assert_eq!(binary_temporal(&[0x03, 1, 2, 3], "TIME"), None);
    }

    #[test]
    fn null_serializes_as_json_null_not_the_string_null() {
        let j = serde_json::to_string(&CellValue::Null).unwrap();
        assert_eq!(j, "null");
        let s = serde_json::to_string(&CellValue::Text("NULL".into())).unwrap();
        assert_eq!(s, "\"NULL\"");
    }

    #[test]
    fn untagged_cells_serialize_as_bare_scalars() {
        assert_eq!(serde_json::to_string(&CellValue::Int(42)).unwrap(), "42");
        assert_eq!(
            serde_json::to_string(&CellValue::Bool(true)).unwrap(),
            "true"
        );
        // DECIMAL arrives as Text so it keeps full precision in the grid.
        assert_eq!(
            serde_json::to_string(&CellValue::Text("10.00".into())).unwrap(),
            "\"10.00\""
        );
    }

    #[test]
    fn type_hints_cover_the_common_families() {
        assert_eq!(type_hint("BIGINT"), TypeHint::Numeric);
        assert_eq!(type_hint("bigint unsigned"), TypeHint::Numeric);
        assert_eq!(type_hint("VARCHAR"), TypeHint::Text);
        assert_eq!(type_hint("JSON"), TypeHint::Text);
        assert_eq!(type_hint("DATETIME"), TypeHint::Temporal);
        assert_eq!(type_hint("LONGBLOB"), TypeHint::Binary);
        assert_eq!(type_hint("SOMETHING_NEW"), TypeHint::Text);
    }

    #[test]
    fn js_safe_boundary_is_where_we_switch_to_text() {
        assert_eq!(JS_SAFE_INT, (1i64 << 53) - 1);
    }
}
