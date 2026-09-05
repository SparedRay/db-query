//! Command surface and state registration.

pub mod decode;
pub mod exec;
pub mod files;
pub mod lint;
pub mod schema;
pub mod session;
pub mod split;

use tauri::State;

use exec::ScriptResult;
use files::{FileTypeSpec, OpenedFile, SaveOutcome, SavedFile};
use lint::Diagnostic;
use schema::{ColumnInfo, TableRef};
use session::{AppState, ConnConfig, ConnInfo, TabStatus};
use split::SplitOutput;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
async fn connect(state: State<'_, AppState>, config: ConnConfig) -> Result<ConnInfo, String> {
    session::connect(&state, config).await
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    session::disconnect(&state).await
}

// ---------------------------------------------------------------- tab lifecycle

#[tauri::command]
async fn open_tab(state: State<'_, AppState>, tab_id: String) -> Result<(), String> {
    session::open_tab(&state, &tab_id).await
}

#[tauri::command]
async fn close_tab(state: State<'_, AppState>, tab_id: String) -> Result<(), String> {
    session::close_tab(&state, &tab_id).await
}

// ----------------------------------------------------------------------- per tab

#[tauri::command]
async fn use_database(
    state: State<'_, AppState>,
    tab_id: String,
    db: String,
) -> Result<(), String> {
    session::use_database(&state, &tab_id, &db).await
}

#[tauri::command]
async fn tab_status(state: State<'_, AppState>, tab_id: String) -> Result<TabStatus, String> {
    session::tab_status(&state, &tab_id).await
}

#[tauri::command]
async fn run_script(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
    auto_limit: bool,
    timeout_secs: Option<u64>,
) -> Result<ScriptResult, String> {
    exec::run_script(&state, &tab_id, &sql, auto_limit, timeout_secs).await
}

#[tauri::command]
async fn cancel_query(state: State<'_, AppState>, tab_id: String) -> Result<(), String> {
    session::cancel_query(&state, &tab_id).await
}

/// Advisory diagnostics. Never gates execution — the frontend renders these as
/// squiggles and nothing more.
///
/// Schema-aware checks resolve against THIS tab's active database, since each
/// tab has its own `USE`. An empty schema means the cache is not warm yet,
/// which suppresses those checks rather than reporting every table as unknown.
#[tauri::command]
async fn lint_sql(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
) -> Result<Vec<Diagnostic>, String> {
    let db = match session::tab_if_open(&state, &tab_id).await {
        Some(tab) => tab.current_db.lock().await.clone(),
        None => None,
    };
    let schema = schema::lint_schema(&state, db.as_deref()).await;
    Ok(lint::lint(&sql, &schema))
}

// ------------------------------------------------------- shared (meta connection)

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
    schema::refresh(&state, &db).await
}

// ------------------------------------------------------------------- stateless

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

// ----------------------------------------------------------------------- files

/// The file types we can open. The frontend uses this for its own affordances;
/// the open dialog builds its filters from the same registry, so adding an
/// engine later means editing one list.
#[tauri::command]
fn supported_file_types() -> Vec<FileTypeSpec> {
    files::supported_file_types()
}

/// Pick a file and read it in a single round trip.
///
/// The dialog is driven from Rust rather than JavaScript so there is no window
/// where the frontend holds a path it has not yet been able to read.
#[tauri::command]
async fn open_file_dialog(app: tauri::AppHandle) -> Result<Option<OpenedFile>, String> {
    let mut builder = app.dialog().file();
    for spec in files::supported_file_types() {
        let exts: Vec<&str> = spec.extensions.iter().map(String::as_str).collect();
        builder = builder.add_filter(&spec.label, &exts);
    }
    builder = builder.add_filter("All files", &["*"]);

    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.pick_file(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx
        .await
        .map_err(|_| "The file dialog closed unexpectedly.".to_string())?;

    let Some(file_path) = picked else {
        return Ok(None); // user cancelled
    };
    let path = file_path
        .into_path()
        .map_err(|e| format!("Unsupported file path: {e}"))?;
    files::read_file(&path).map(Some)
}

/// Read a known path — used by drag-and-drop, and by a future recent-files list.
#[tauri::command]
fn read_file(path: String) -> Result<OpenedFile, String> {
    files::read_file(std::path::Path::new(&path))
}

/// Save over an existing file. `expect_mtime` is what we saw when the file was
/// opened; a mismatch returns `Conflict` instead of overwriting someone's work.
#[tauri::command]
fn save_file(
    path: String,
    contents: String,
    expect_mtime: Option<u64>,
    line_ending: String,
) -> Result<SaveOutcome, String> {
    files::save_file(
        std::path::Path::new(&path),
        &contents,
        expect_mtime,
        &line_ending,
    )
}

/// Save As: pick a destination and write it, returning the new identity so the
/// tab can retitle itself.
#[tauri::command]
async fn save_file_dialog(
    app: tauri::AppHandle,
    suggested_name: String,
    contents: String,
    line_ending: String,
) -> Result<Option<SavedFile>, String> {
    let mut builder = app.dialog().file().set_file_name(&suggested_name);
    for spec in files::supported_file_types() {
        let exts: Vec<&str> = spec.extensions.iter().map(String::as_str).collect();
        builder = builder.add_filter(&spec.label, &exts);
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.save_file(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx
        .await
        .map_err(|_| "The file dialog closed unexpectedly.".to_string())?;

    let Some(file_path) = picked else {
        return Ok(None); // user cancelled
    };
    let path = file_path
        .into_path()
        .map_err(|e| format!("Unsupported file path: {e}"))?;

    // The native dialog already handled any overwrite confirmation, so there is
    // no mtime expectation to check here.
    match files::save_file(&path, &contents, None, &line_ending)? {
        SaveOutcome::Saved { mtime_ms } => Ok(Some(SavedFile {
            path: path.to_string_lossy().into_owned(),
            name: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("untitled")
                .to_string(),
            mtime_ms,
        })),
        SaveOutcome::Conflict { .. } => {
            Err("The destination changed while saving. Try again.".into())
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            open_tab,
            close_tab,
            use_database,
            tab_status,
            run_script,
            cancel_query,
            lint_sql,
            list_tables,
            list_columns,
            refresh_schema,
            split_sql,
            statement_at_cursor,
            supported_file_types,
            open_file_dialog,
            read_file,
            save_file,
            save_file_dialog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
