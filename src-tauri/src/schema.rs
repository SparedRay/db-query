//! information_schema introspection, cached per database.
//!
//! Two rules that both come from real failures:
//!
//!   * **Every query is filtered by schema.** Enumerating information_schema
//!     globally is what makes clients hang on servers with thousands of tables.
//!   * **Every projected column is aliased.** MySQL 8 returns information_schema
//!     column names UPPERCASED regardless of how the query is written, and sqlx
//!     matches names case-sensitively. Without aliases every row is silently
//!     dropped and the schema tree renders empty with no error at all.
//!
//! Introspection runs on the shared **meta** connection, never on a tab's exec
//! connection: expanding the tree must not block behind a running query, and it
//! must not contend with cancellation.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use serde::Serialize;
use sqlx::Row;

use crate::session::{friendly, server, AppState};

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
pub async fn refresh(state: &AppState, db: &str) -> Result<(), String> {
    let server = server(state).await?;
    server.schema_cache.lock().await.remove(db);
    Ok(())
}

pub async fn list_tables(state: &AppState, db: &str) -> Result<Vec<TableRef>, String> {
    let server = server(state).await?;

    if let Some(cached) = server
        .schema_cache
        .lock()
        .await
        .get(db)
        .and_then(|s| s.tables.clone())
    {
        return Ok(cached);
    }

    let rows = {
        let mut meta = server.meta.lock().await;
        server.introspection_count.fetch_add(1, Ordering::SeqCst);
        sqlx::query(
            "SELECT table_name AS name, table_type AS kind \
             FROM information_schema.tables \
             WHERE table_schema = ? \
             ORDER BY table_name",
        )
        .bind(db)
        .fetch_all(&mut *meta)
        .await
        .map_err(|e| friendly(&e))?
    };

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

    server
        .schema_cache
        .lock()
        .await
        .entry(db.to_string())
        .or_default()
        .tables = Some(tables.clone());
    Ok(tables)
}

pub async fn list_columns(
    state: &AppState,
    db: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
    let server = server(state).await?;

    if let Some(cached) = server
        .schema_cache
        .lock()
        .await
        .get(db)
        .and_then(|s| s.columns.get(table))
        .cloned()
    {
        return Ok(cached);
    }

    let rows = {
        let mut meta = server.meta.lock().await;
        server.introspection_count.fetch_add(1, Ordering::SeqCst);
        sqlx::query(
            "SELECT column_name AS name, column_type AS data_type, \
                    is_nullable AS nullable, column_key AS col_key \
             FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY ordinal_position",
        )
        .bind(db)
        .bind(table)
        .fetch_all(&mut *meta)
        .await
        .map_err(|e| friendly(&e))?
    };

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

    server
        .schema_cache
        .lock()
        .await
        .entry(db.to_string())
        .or_default()
        .columns
        .insert(table.to_string(), columns.clone());
    Ok(columns)
}

/// The lint schema for a tab's active database: lowercased table -> columns.
/// Empty means "cache not warm yet", which suppresses all schema-aware lint
/// checks rather than reporting every table as unknown.
pub async fn lint_schema(state: &AppState, db: Option<&str>) -> crate::lint::LintSchema {
    let Some(db) = db else {
        return Default::default();
    };
    let Ok(server) = server(state).await else {
        return Default::default();
    };
    let cache = server.schema_cache.lock().await;
    cache
        .get(db)
        .map(|s| {
            s.columns
                .iter()
                .map(|(t, cols)| {
                    (
                        t.to_ascii_lowercase(),
                        cols.iter().map(|c| c.name.to_ascii_lowercase()).collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}
