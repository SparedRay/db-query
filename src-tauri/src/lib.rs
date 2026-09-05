//! Command surface and state registration.

pub mod decode;
pub mod exec;
pub mod lint;
pub mod schema;
pub mod session;
pub mod split;

use serde::Serialize;
use tauri::State;

use exec::ScriptResult;
use lint::Diagnostic;
use schema::{ColumnInfo, TableRef};
use session::{AppState, ConnConfig, ConnInfo};
use split::SplitOutput;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusInfo {
    connected: bool,
    host_label: Option<String>,
    current_database: Option<String>,
}

#[tauri::command]
async fn connect(state: State<'_, AppState>, config: ConnConfig) -> Result<ConnInfo, String> {
    session::connect(&state, config).await
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    session::disconnect(&state).await
}

#[tauri::command]
async fn use_database(state: State<'_, AppState>, db: String) -> Result<(), String> {
    session::use_database(&state, &db).await
}

#[tauri::command]
async fn status(state: State<'_, AppState>) -> Result<StatusInfo, String> {
    let ctl = state.ctl.lock().await;
    Ok(match ctl.as_ref() {
        Some(c) => StatusInfo {
            connected: true,
            host_label: Some(c.host_label.clone()),
            current_database: c.current_db.clone(),
        },
        None => StatusInfo {
            connected: false,
            host_label: None,
            current_database: None,
        },
    })
}

#[tauri::command]
async fn list_tables(state: State<'_, AppState>, db: String) -> Result<Vec<TableRef>, String> {
    schema::list_tables(&state, &db).await
}

#[tauri::command]
async fn list_columns(
    state: State<'_, AppState>,
    db: String,
    table: String,
) -> Result<Vec<ColumnInfo>, String> {
    schema::list_columns(&state, &db, &table).await
}

#[tauri::command]
async fn refresh_schema(state: State<'_, AppState>, db: String) -> Result<(), String> {
    schema::refresh(&state, &db).await;
    Ok(())
}

/// The frontend never parses SQL. When it needs statement boundaries it asks
/// here, so cursor detection and execution always agree.
#[tauri::command]
fn split_sql(sql: String) -> Result<SplitOutput, String> {
    Ok(split::split(&sql))
}

/// Statement under the cursor, resolved entirely in Rust. The frontend sends a
/// BYTE offset and gets back the statement text — it never slices the buffer or
/// reimplements the boundary rules, so execution and cursor detection cannot
/// drift apart.
#[tauri::command]
fn statement_at_cursor(sql: String, cursor: usize) -> Result<Option<String>, String> {
    let out = split::split(&sql);
    if out.delimiter_detected {
        // Terminator is user-defined; the whole buffer is the unit of work.
        return Ok(Some(sql));
    }
    Ok(split::statement_at(&out.statements, cursor)
        .and_then(|i| out.statements.get(i))
        .map(|s| sql[s.start..s.end].to_string()))
}

#[tauri::command]
async fn run_script(
    state: State<'_, AppState>,
    sql: String,
    auto_limit: bool,
    timeout_secs: Option<u64>,
) -> Result<ScriptResult, String> {
    exec::run_script(&state, &sql, auto_limit, timeout_secs).await
}

/// Advisory diagnostics. Never gates execution — the frontend renders these as
/// squiggles and nothing more.
///
/// The schema half is built from whatever the cache already holds for the
/// active database. An empty map means "not loaded yet", which suppresses all
/// schema-aware checks rather than reporting every table as unknown.
#[tauri::command]
async fn lint_sql(state: State<'_, AppState>, sql: String) -> Result<Vec<Diagnostic>, String> {
    let schema = {
        let guard = state.ctl.lock().await;
        match guard.as_ref() {
            Some(ctl) => ctl
                .current_db
                .as_ref()
                .and_then(|db| ctl.schema_cache.get(db))
                .map(|db_schema| {
                    db_schema
                        .columns
                        .iter()
                        .map(|(table, cols)| {
                            (
                                table.to_ascii_lowercase(),
                                cols.iter().map(|c| c.name.to_ascii_lowercase()).collect(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None => lint::LintSchema::new(),
        }
    };
    Ok(lint::lint(&sql, &schema))
}

#[tauri::command]
async fn cancel_query(state: State<'_, AppState>) -> Result<(), String> {
    session::cancel_query(&state).await
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            use_database,
            status,
            list_tables,
            list_columns,
            refresh_schema,
            split_sql,
            statement_at_cursor,
            run_script,
            lint_sql,
            cancel_query,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
