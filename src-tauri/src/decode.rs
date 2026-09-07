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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellValue {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
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
    if raw.is_null() {
        return CellValue::null();
    }

    let sql_type = raw.type_info().name().to_ascii_uppercase();

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
        "DATE" | "TIME" | "DATETIME" | "TIMESTAMP" | "YEAR" => temporal_text(row, idx),
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
                    None => CellValue::Text(format!("<binary, {} bytes>", b.len())),
                },
                Err(_) => CellValue::Text("<binary>".into()),
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

fn temporal_text(row: &MySqlRow, idx: usize) -> CellValue {
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
