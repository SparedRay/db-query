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

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::session::{friendly, server, AppState, ServerConn};

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

/// Procedure or function.
///
/// An enum rather than the free-form `String` that `TableRef.kind` uses, and
/// deliberately: this value is interpolated straight into `SHOW CREATE …` and
/// `DROP …`. A string there would be an injection vector reachable from
/// whatever the server returned. Two variants cannot be anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutineKind {
    Procedure,
    Function,
}

impl RoutineKind {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Procedure => "PROCEDURE",
            Self::Function => "FUNCTION",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "PROCEDURE" => Some(Self::Procedure),
            "FUNCTION" => Some(Self::Function),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineParam {
    pub name: String,
    /// "IN" | "OUT" | "INOUT". Functions report no mode; they are all IN.
    pub mode: String,
    pub data_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineRef {
    pub name: String,
    pub kind: RoutineKind,
    /// Return type. `Some` for functions, `None` for procedures.
    pub returns: Option<String>,
    pub params: Vec<RoutineParam>,
}

#[derive(Debug, Default, Clone)]
pub struct DbSchema {
    pub tables: Option<Vec<TableRef>>,
    pub columns: HashMap<String, Vec<ColumnInfo>>,
    pub routines: Option<Vec<RoutineRef>>,
}

/// How many tables get their columns loaded when describing a database to the
/// assistant.
///
/// A cap rather than "all of them" because columns are fetched per table: a
/// 500-table warehouse would be 500 round trips before the first word of an
/// answer. Sixty is roughly a screenful of `render_schema` and a few thousand
/// tokens — enough that the model stops guessing, small enough that the wait is
/// not noticed. Table *names* are never capped; they cost one query for all of
/// them, and knowing a table exists is most of the value.
pub const ASSISTANT_TABLE_BUDGET: usize = 60;

/// What [`warm_for_assistant`] managed to load.
#[derive(Debug, Clone, Copy, Default)]
pub struct Warmed {
    /// Tables and views this database has.
    pub tables: usize,
    /// How many of them now have their columns cached.
    pub detailed: usize,
}

/// Load enough of a database's shape to describe it to the assistant.
///
/// # Why this is not "running queries automatically"
///
/// It runs no user SQL and executes nothing the user wrote. It calls the same
/// introspection the schema tree calls when you expand a node — the engine's
/// `tables` and `columns` — which is the app describing its own connection, not
/// the model reaching a database. Nothing here is recorded as history, nothing
/// appears in the grid, and the assistant still cannot execute a statement.
///
/// Without it the model is told "columns not loaded" for every table nobody
/// happened to click, and so it invents column names — which is the one failure
/// mode that makes a SQL assistant worse than no assistant.
///
/// Results land in the ordinary schema cache, so this is paid once per database
/// per connection, and the tree gets the benefit too.
pub async fn warm_for_assistant(
    state: &AppState,
    connection_id: &str,
    db: &str,
    budget: usize,
) -> Result<Warmed, String> {
    let tables = list_tables(state, connection_id, db).await?;
    let mut warmed = Warmed {
        tables: tables.len(),
        detailed: 0,
    };

    for t in tables.iter().take(budget) {
        // Per table, and tolerant: a view whose definition no longer resolves
        // cannot be described, and that must cost us that one table rather than
        // the whole schema. `list_columns` is cache-first, so a table the tree
        // already expanded costs nothing here.
        if list_columns(state, connection_id, db, &t.name)
            .await
            .is_ok()
        {
            warmed.detailed += 1;
        }
    }
    Ok(warmed)
}

/// Drop any cached introspection for `db`, so the next expand refetches.
pub async fn refresh(state: &AppState, connection_id: &str, db: &str) -> Result<(), String> {
    let server = server(state, connection_id).await?;
    server.schema_cache.lock().await.remove(db);
    Ok(())
}

pub(crate) async fn mysql_list_tables(
    server: &ServerConn,
    db: &str,
) -> Result<Vec<TableRef>, String> {
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
        let mut meta = server.mysql_meta().await?;
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

pub(crate) async fn mysql_list_columns(
    server: &ServerConn,
    db: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
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
        let mut meta = server.mysql_meta().await?;
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

/// Procedures and functions in one database, with their parameters.
///
/// Two queries rather than a join, and one `parameters` query for the whole
/// schema rather than one per routine — a server with hundreds of routines
/// should still cost two round trips.
///
/// **`ordinal_position = 0` is the function's return value, not a parameter.**
/// It comes back with a NULL name and NULL mode, so including it would put a
/// nameless argument at the front of every generated `SELECT fn(...)` call.
pub(crate) async fn mysql_list_routines(
    server: &ServerConn,
    db: &str,
) -> Result<Vec<RoutineRef>, String> {
    if let Some(cached) = server
        .schema_cache
        .lock()
        .await
        .get(db)
        .and_then(|s| s.routines.clone())
    {
        return Ok(cached);
    }

    let (routine_rows, param_rows) = {
        let mut meta = server.mysql_meta().await?;
        server.introspection_count.fetch_add(2, Ordering::SeqCst);
        let routines = sqlx::query(
            "SELECT routine_name AS name, routine_type AS kind, \
                    dtd_identifier AS returns_type \
             FROM information_schema.routines \
             WHERE routine_schema = ? \
             ORDER BY routine_name",
        )
        .bind(db)
        .fetch_all(&mut *meta)
        .await
        .map_err(|e| friendly(&e))?;

        let params = sqlx::query(
            "SELECT specific_name AS routine, parameter_mode AS mode, \
                    parameter_name AS name, dtd_identifier AS data_type \
             FROM information_schema.parameters \
             WHERE specific_schema = ? AND ordinal_position > 0 \
             ORDER BY specific_name, ordinal_position",
        )
        .bind(db)
        .fetch_all(&mut *meta)
        .await
        .map_err(|e| friendly(&e))?;

        (routines, params)
    };

    let mut by_routine: HashMap<String, Vec<RoutineParam>> = HashMap::new();
    for r in param_rows {
        let Ok(routine) = r.try_get::<String, _>("routine") else {
            continue;
        };
        by_routine.entry(routine).or_default().push(RoutineParam {
            name: r.try_get::<String, _>("name").unwrap_or_default(),
            // Functions report no mode. They are all IN, so say so rather than
            // leaving a blank the caller has to interpret.
            mode: r
                .try_get::<Option<String>, _>("mode")
                .ok()
                .flatten()
                .unwrap_or_else(|| "IN".into()),
            data_type: r.try_get::<String, _>("data_type").unwrap_or_default(),
        });
    }

    let routines: Vec<RoutineRef> = routine_rows
        .into_iter()
        .filter_map(|r| {
            let name: String = r.try_get("name").ok()?;
            // A routine whose type we cannot read is one we could not safely
            // generate SQL for, so it is dropped rather than guessed at.
            let kind = RoutineKind::parse(&r.try_get::<String, _>("kind").ok()?)?;
            let returns = match kind {
                RoutineKind::Function => r
                    .try_get::<Option<String>, _>("returns_type")
                    .ok()
                    .flatten()
                    .filter(|t| !t.is_empty()),
                RoutineKind::Procedure => None,
            };
            let params = by_routine.remove(&name).unwrap_or_default();
            Some(RoutineRef {
                name,
                kind,
                returns,
                params,
            })
        })
        .collect();

    server
        .schema_cache
        .lock()
        .await
        .entry(db.to_string())
        .or_default()
        .routines = Some(routines.clone());
    Ok(routines)
}

/// The server's own `CREATE TABLE` for a real table.
///
/// Preferred over deriving one from result metadata whenever the rows came from
/// a table, because it is **exact**: the MySQL protocol carries no lengths,
/// keys, defaults or nullability, so a derived schema is always a widened
/// approximation. This is what makes an exported script recreate the original
/// rather than something shaped like it.
pub(crate) async fn mysql_table_ddl(
    server: &ServerConn,
    db: &str,
    table: &str,
) -> Result<String, String> {
    let qualified = crate::sqlgen::qualify(db, table)?;

    let row = {
        let mut meta = server.mysql_meta().await?;
        server.introspection_count.fetch_add(1, Ordering::SeqCst);
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SHOW CREATE TABLE {qualified}"
        )))
        .fetch_one(&mut *meta)
        .await
        .map_err(|e| friendly(&e))?
    };

    // A view answers `SHOW CREATE TABLE` with a "Create View" column instead.
    let ddl: Option<String> = row
        .try_get("Create Table")
        .or_else(|_| row.try_get("Create View"))
        .map_err(|e| friendly(&e))?;

    ddl.filter(|d| !d.trim().is_empty())
        .map(|d| format!("{};\n", d.trim_end().trim_end_matches(';')))
        .ok_or_else(|| format!("The server returned no definition for {qualified}."))
}

/// The re-runnable creation script for one routine.
///
/// `SHOW CREATE …` cannot take a bind parameter — it takes an identifier — so
/// the name is quoted rather than bound. That is what `quote_ident` is for, and
/// why `RoutineKind` is an enum: the only two words that can reach the keyword
/// slot are the two spelled out in its `keyword()`.
pub(crate) async fn mysql_routine_ddl(
    server: &ServerConn,
    db: &str,
    name: &str,
    kind: RoutineKind,
) -> Result<String, String> {
    let kw = kind.keyword();
    let qualified = crate::sqlgen::qualify(db, name)?;
    let sql = format!("SHOW CREATE {kw} {qualified}");

    let row = {
        let mut meta = server.mysql_meta().await?;
        server.introspection_count.fetch_add(1, Ordering::SeqCst);
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut *meta)
            .await
            .map_err(|e| friendly(&e))?
    };

    // The body column is named after the routine kind: "Create Procedure" or
    // "Create Function".
    let column = match kind {
        RoutineKind::Procedure => "Create Procedure",
        RoutineKind::Function => "Create Function",
    };

    // NULL here is the privilege case, not a missing routine: MySQL returns the
    // row with an empty body when the caller may not see the definition. Saying
    // so beats handing back an empty tab.
    let body: Option<String> = row.try_get(column).map_err(|e| friendly(&e))?;
    let body = body.filter(|b| !b.trim().is_empty()).ok_or_else(|| {
        format!(
            "The server did not return a definition for {kw} {qualified}. This usually \
             means the account lacks the privilege to see the routine body."
        )
    })?;

    crate::sqlgen::routine_script(db, name, kind, &body)
}

/// The lint schema for a tab's active database: lowercased table -> columns.
/// Empty means "cache not warm yet", which suppresses all schema-aware lint
/// checks rather than reporting every table as unknown.
///
/// Takes the server directly rather than a connection id, because `lint_sql`
/// resolves it from the tab — which is the only source that cannot be wrong.
pub async fn lint_schema(server: &ServerConn, db: Option<&str>) -> crate::lint::LintSchema {
    let Some(db) = db else {
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

// ------------------------------------------------------------ engine dispatch
//
// The command layer calls these; they ask the connection's engine. MySQL's
// implementations are the `mysql_*` functions above, unchanged — the split is a
// dispatch layer, not a rewrite, which is what let the whole existing suite run
// untouched through this change.

pub async fn list_tables(
    state: &AppState,
    connection_id: &str,
    db: &str,
) -> Result<Vec<TableRef>, String> {
    let server = server(state, connection_id).await?;
    server.engine.tables(&server, db).await
}

pub async fn list_columns(
    state: &AppState,
    connection_id: &str,
    db: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
    let server = server(state, connection_id).await?;
    server.engine.columns(&server, db, table).await
}

pub async fn list_routines(
    state: &AppState,
    connection_id: &str,
    db: &str,
) -> Result<Vec<RoutineRef>, String> {
    let server = server(state, connection_id).await?;
    server.engine.routines(&server, db).await
}

pub async fn table_ddl(
    state: &AppState,
    connection_id: &str,
    db: &str,
    table: &str,
) -> Result<String, String> {
    let server = server(state, connection_id).await?;
    server.engine.table_ddl(&server, db, table).await
}

pub async fn routine_ddl(
    state: &AppState,
    connection_id: &str,
    db: &str,
    name: &str,
    kind: RoutineKind,
) -> Result<String, String> {
    let server = server(state, connection_id).await?;
    server.engine.routine_ddl(&server, db, name, kind).await
}
