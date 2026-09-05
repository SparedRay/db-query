//! information_schema introspection, cached per database.
//!
//! Every query is filtered by schema. Enumerating information_schema globally
//! is what makes clients hang on servers with thousands of tables.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::Row;

use crate::session::{friendly, AppState};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRef {
    pub name: String,
    /// "BASE TABLE" | "VIEW"
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub key: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct DbSchema {
    pub tables: Option<Vec<TableRef>>,
    pub columns: HashMap<String, Vec<ColumnInfo>>,
}

/// Drop any cached introspection for `db`, so the next expand refetches.
pub async fn refresh(state: &AppState, db: &str) {
    if let Some(ctl) = state.ctl.lock().await.as_mut() {
        ctl.schema_cache.remove(db);
    }
}

pub async fn list_tables(state: &AppState, db: &str) -> Result<Vec<TableRef>, String> {
    if let Some(ctl) = state.ctl.lock().await.as_ref() {
        if let Some(cached) = ctl.schema_cache.get(db).and_then(|s| s.tables.clone()) {
            return Ok(cached);
        }
    }

    let mut guard = state.exec.lock().await;
    let conn = guard.as_mut().ok_or("Not connected.")?;
    // Alias every column. MySQL 8 returns information_schema column names
    // UPPERCASED regardless of how the query is written, and sqlx matches
    // names case-sensitively — without aliases every row is silently dropped
    // and the schema tree renders empty with no error.
    let rows = sqlx::query(
        "SELECT table_name AS name, table_type AS kind \
         FROM information_schema.tables \
         WHERE table_schema = ? \
         ORDER BY table_name",
    )
    .bind(db)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| friendly(&e))?;
    drop(guard);

    let tables: Vec<TableRef> = rows
        .into_iter()
        .filter_map(|r| {
            Some(TableRef {
                name: r.try_get::<String, _>("name").ok()?,
                kind: r
                    .try_get::<String, _>("kind")
                    .unwrap_or_else(|_| "BASE TABLE".into()),
            })
        })
        .collect();

    if let Some(ctl) = state.ctl.lock().await.as_mut() {
        ctl.schema_cache.entry(db.to_string()).or_default().tables = Some(tables.clone());
    }
    Ok(tables)
}

pub async fn list_columns(
    state: &AppState,
    db: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
    if let Some(ctl) = state.ctl.lock().await.as_ref() {
        if let Some(cached) = ctl
            .schema_cache
            .get(db)
            .and_then(|s| s.columns.get(table))
            .cloned()
        {
            return Ok(cached);
        }
    }

    let mut guard = state.exec.lock().await;
    let conn = guard.as_mut().ok_or("Not connected.")?;
    // Aliased for the same reason as list_tables above.
    let rows = sqlx::query(
        "SELECT column_name AS name, column_type AS data_type, \
                is_nullable AS nullable, column_key AS col_key \
         FROM information_schema.columns \
         WHERE table_schema = ? AND table_name = ? \
         ORDER BY ordinal_position",
    )
    .bind(db)
    .bind(table)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| friendly(&e))?;
    drop(guard);

    let columns: Vec<ColumnInfo> = rows
        .into_iter()
        .filter_map(|r| {
            let key: String = r.try_get("col_key").unwrap_or_default();
            Some(ColumnInfo {
                name: r.try_get::<String, _>("name").ok()?,
                data_type: r.try_get::<String, _>("data_type").unwrap_or_default(),
                nullable: r
                    .try_get::<String, _>("nullable")
                    .map(|v| v.eq_ignore_ascii_case("YES"))
                    .unwrap_or(true),
                key: (!key.is_empty()).then_some(key),
            })
        })
        .collect();

    if let Some(ctl) = state.ctl.lock().await.as_mut() {
        ctl.schema_cache
            .entry(db.to_string())
            .or_default()
            .columns
            .insert(table.to_string(), columns.clone());
    }
    Ok(columns)
}
