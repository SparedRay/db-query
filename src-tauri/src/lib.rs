//! Command surface and state registration.

pub mod decode;
pub mod exec;
pub mod files;
pub mod lint;
pub mod profiles;
pub mod schema;
pub mod secrets;
pub mod session;
pub mod split;

use tauri::State;

use exec::ScriptResult;
use files::{FileTypeSpec, OpenedFile, SaveOutcome, SavedFile};
use lint::Diagnostic;
use schema::{ColumnInfo, TableRef};
use secrets::Secret;
use session::{AppState, ConnInfo, ConnProfile, ConnectionStatus, ProfileView, TabStatus};
use split::SplitOutput;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

// -------------------------------------------------------------- saved profiles

/// The saved-connection list as the UI receives it. Separate from
/// [`profiles::LoadOutcome`] on purpose: that one is what came off disk, this
/// one adds the keychain-derived `rememberPassword` the frontend needs.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileListOutcome {
    profiles: Vec<ProfileView>,
    warning: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveProfileOutcome {
    profile: ProfileView,
    /// Set when the profile saved but the password did not. The UI says so and
    /// falls back to prompting, rather than pretending it worked.
    password_warning: Option<String>,
    password_stored: bool,
}

fn config_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("Cannot locate the config directory: {e}"))
}

#[tauri::command]
fn list_profiles(app: tauri::AppHandle) -> Result<ProfileListOutcome, String> {
    let dir = config_dir(&app)?;
    let out = profiles::load(&dir);
    // `remember_password` is derived from the keychain, never read from the
    // file — so it reports the truth on whatever machine the file is opened on.
    // It has to be attached *here*, on the wire type: without it the UI cannot
    // tell a remembered connection from one that has never been given a
    // password, and prompts for both on every launch.
    Ok(ProfileListOutcome {
        profiles: out
            .profiles
            .into_iter()
            .map(|p| {
                let stored = secrets::has_stored(&p.id);
                ProfileView::new(p, stored)
            })
            .collect(),
        warning: out.warning,
    })
}

/// Save a profile, and optionally its password.
///
/// `password` is a three-way instruction, not a value to store blindly:
///   * `Some(p)` with `p` non-empty — remember this password
///   * `Some("")` — forget any stored password
///   * `None` — leave whatever is stored alone (editing a profile without
///     retyping the password)
#[tauri::command]
fn save_profile(
    app: tauri::AppHandle,
    profile: ConnProfile,
    password: Option<String>,
) -> Result<SaveProfileOutcome, String> {
    let dir = config_dir(&app)?;
    let mut existing = profiles::load(&dir).profiles;

    profiles::upsert(&mut existing, profile.clone());
    profiles::save_all(&dir, &existing)?;

    let mut password_warning = None;
    let mut password_stored = secrets::has_stored(&profile.id);

    match password {
        Some(p) if p.is_empty() => {
            if let Err(e) = secrets::delete(&profile.id) {
                password_warning = Some(e.to_string());
            }
            password_stored = false;
        }
        Some(p) => {
            let secret = Secret::new(p);
            match secrets::store(&profile.id, &secret) {
                Ok(()) => password_stored = true,
                Err(e) => {
                    password_warning = Some(e.to_string());
                    password_stored = false;
                }
            }
        }
        None => {}
    }

    Ok(SaveProfileOutcome {
        profile: ProfileView::new(profile, password_stored),
        password_warning,
        password_stored,
    })
}

/// Delete a profile **and its stored password**. An orphaned secret outlives
/// the thing that explained what it was for, so a failure to remove it is
/// reported rather than swallowed.
#[tauri::command]
fn delete_profile(app: tauri::AppHandle, id: String) -> Result<Option<String>, String> {
    let dir = config_dir(&app)?;
    let mut existing = profiles::load(&dir).profiles;
    existing.retain(|p| p.id != id);
    profiles::save_all(&dir, &existing)?;
    Ok(secrets::delete(&id).err().map(|e| e.to_string()))
}

#[tauri::command]
fn has_stored_password(id: String) -> Result<bool, String> {
    Ok(secrets::has_stored(&id))
}

// ------------------------------------------------------------------ connections

/// Open a connection. Ad hoc or from a saved profile — identical either way;
/// persistence is Phase 2's concern, not this command's.
#[tauri::command]
async fn connect(
    state: State<'_, AppState>,
    profile: ConnProfile,
    password: String,
) -> Result<ConnInfo, String> {
    session::connect(&state, profile, password).await
}

/// Connect using a profile's remembered password.
///
/// Errors when nothing is stored: the caller then prompts, which is also the
/// path taken on a machine with no credential store at all.
#[tauri::command]
async fn connect_saved(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> Result<ConnInfo, String> {
    let dir = config_dir(&app)?;
    let profile = profiles::load(&dir)
        .profiles
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| "No saved connection with that id.".to_string())?;

    let secret = secrets::load(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No password is stored for this connection.".to_string())?;

    session::connect(&state, profile, secret.expose().to_string()).await
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    session::disconnect(&state, &connection_id).await
}

#[tauri::command]
async fn disconnect_all(state: State<'_, AppState>) -> Result<(), String> {
    session::disconnect_all(&state).await
}

#[tauri::command]
async fn list_connections(state: State<'_, AppState>) -> Result<Vec<ConnectionStatus>, String> {
    Ok(session::list_connections(&state).await)
}

// ---------------------------------------------------------------- tab lifecycle

/// Attach a tab to a connection. A tab is bound for life, so nothing it runs
/// can reach a different server.
#[tauri::command]
async fn open_tab(
    state: State<'_, AppState>,
    connection_id: String,
    tab_id: String,
) -> Result<(), String> {
    session::open_tab(&state, &connection_id, &tab_id).await
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
/// The schema comes from the tab's own connection and its own active database.
/// Resolving it through the tab rather than a caller-supplied connection id is
/// deliberate: it is the one source that cannot name the wrong server.
#[tauri::command]
async fn lint_sql(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
) -> Result<Vec<Diagnostic>, String> {
    let Some(tab) = session::tab_if_open(&state, &tab_id).await else {
        // An unattached tab still gets Tier 1 checks; only the schema-aware
        // ones need a connection.
        return Ok(lint::lint(&sql, &lint::LintSchema::new()));
    };
    let db = tab.current_db.lock().await.clone();
    let schema = schema::lint_schema(&tab.server, db.as_deref()).await;
    Ok(lint::lint(&sql, &schema))
}

// ------------------------------------------------------- schema (meta connection)

#[tauri::command]
async fn list_tables(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
) -> Result<Vec<TableRef>, String> {
    schema::list_tables(&state, &connection_id, &db).await
}

#[tauri::command]
async fn list_columns(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
    table: String,
) -> Result<Vec<ColumnInfo>, String> {
    schema::list_columns(&state, &connection_id, &db, &table).await
}

#[tauri::command]
async fn refresh_schema(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
) -> Result<(), String> {
    schema::refresh(&state, &connection_id, &db).await
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
            connect_saved,
            disconnect,
            disconnect_all,
            list_connections,
            list_profiles,
            save_profile,
            delete_profile,
            has_stored_password,
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
