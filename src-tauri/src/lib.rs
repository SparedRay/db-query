//! Command surface and state registration.

pub mod assistant;
pub mod decode;
pub mod elastic;
pub mod engine;
pub mod exec;
pub mod export;
pub mod files;
pub mod history;
pub mod httpsql;
pub mod lint;
pub mod mcp;
pub mod mysql;
pub mod profiles;
pub mod schema;
pub mod secrets;
pub mod session;
pub mod split;
pub mod sqlfmt;
pub mod sqlgen;
pub mod update;
pub mod workspace;

use tauri::State;

use decode::CellValue;
use exec::ScriptResult;
use files::{FileTypeSpec, OpenedFile, SaveOutcome, SavedFile};
use lint::Diagnostic;
use schema::{ColumnInfo, RoutineKind, RoutineRef, TableRef};
use secrets::Secret;
use session::{AppState, ConnInfo, ConnProfile, ConnectionStatus, ProfileView, TabStatus};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

/// Shipped as a bundle resource; see `third_party_licenses`. Named once so the
/// packaging test and the command cannot drift apart.
pub const LICENSES_FILE: &str = "THIRD-PARTY-LICENSES.txt";

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

// ------------------------------------------------------------------ session
//
// The open tabs, remembered across restarts. Rust owns the file and nothing
// else: what a tab *is* stays the frontend's business.

#[tauri::command]
fn load_session(app: tauri::AppHandle) -> Result<workspace::LoadOutcome, String> {
    Ok(workspace::load(&config_dir(&app)?))
}

#[tauri::command]
fn save_session(app: tauri::AppHandle, session: workspace::SessionStore) -> Result<(), String> {
    workspace::save(&config_dir(&app)?, &session)
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
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
    auto_limit: bool,
    timeout_secs: Option<u64>,
) -> Result<ScriptResult, String> {
    let result = exec::run_script(&state, &tab_id, &sql, auto_limit, timeout_secs).await?;
    if let Ok(dir) = config_dir(&app) {
        remember(&dir, &state, &tab_id, &result).await;
    }
    Ok(result)
}

/// Write what just ran into the history file.
///
/// **Recorded here rather than inside `exec::run_script`** so that execution
/// owns no opinion about a config directory, and so that a history failure
/// cannot fail a query. Everything is best-effort and silent: the worst case is
/// a statement missing from a list, and interrupting someone's work to say so
/// would be a far worse trade.
///
/// Recorded from Rust rather than from the frontend because this is the single
/// point every execution passes through. The `sql` stored is what the user
/// wrote, never `effective_sql` — an auto-LIMIT we added is ours, and handing
/// it back later as though they had typed it would be a small lie that
/// compounds every time the statement is re-run.
/// Takes a directory rather than an `AppHandle` so the live suite can drive it
/// against a real database — the Tauri layer is the one place a test cannot go,
/// and "does running a script actually record it" is the question that matters.
pub async fn remember(
    dir: &std::path::Path,
    state: &AppState,
    tab_id: &str,
    result: &ScriptResult,
) {
    let Ok(tab) = session::tab(state, tab_id).await else {
        return;
    };
    let connection_id = tab.server.id().to_string();
    let database = tab.current_db.lock().await.clone();

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let proposals = state.proposals.lock().await;
    let entries: Vec<history::Entry> = result
        .statements
        .iter()
        .map(|s| {
            let (status, rows, error) = match &s.outcome {
                exec::Outcome::Rows { rows, .. } => ("ok", Some(rows.len() as u64), None),
                exec::Outcome::Affected { rows } => ("ok", Some(*rows), None),
                exec::Outcome::Error { message } => ("error", None, Some(message.clone())),
            };
            history::Entry {
                at,
                connection_id: connection_id.clone(),
                source: source_of(&proposals, &s.sql),
                database: database.clone(),
                sql: s.sql.clone(),
                kind: serde_json::to_value(s.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "other".into()),
                status: status.into(),
                rows,
                elapsed_ms: s.elapsed_ms,
                error,
            }
        })
        .collect();

    drop(proposals);
    let _ = history::record(dir, &entries);
}

/// Whose statement is this?
///
/// Exact text only. If you edited what the assistant offered before running it,
/// it is yours — which is both the honest answer and the one that needs no
/// fuzzy matching to reach.
fn source_of(proposals: &assistant::Proposals, sql: &str) -> String {
    if proposals.contains(sql) {
        "assistant".into()
    } else {
        "user".into()
    }
}

// ----------------------------------------------------------------- assistant
//
// An assistant that writes SQL for the user to read. It has no tools and no
// connection: see `assistant.rs` for why that is the design rather than a
// limitation.

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AssistantStatus {
    /// True when this provider has a key stored. The key never comes back out.
    has_key: bool,
    /// True when it can actually be used — a local model needs no key.
    ready: bool,
    /// True when nothing leaves the machine, which is worth saying out loud.
    local: bool,
}

#[tauri::command]
fn assistant_status(provider: assistant::Provider, base_url: String) -> AssistantStatus {
    let has_key = secrets::has_stored(provider.key_id());
    let local = provider == assistant::Provider::OpenAiCompatible && assistant::is_local(&base_url);
    AssistantStatus {
        has_key,
        ready: has_key || !provider.requires_key(&base_url),
        local,
    }
}

/// Store or forget a provider's API key. `None` (or empty) forgets it.
///
/// Same three-way shape and the same store as a database password: the OS
/// credential store, never a config file. Keyed per provider, so configuring a
/// local model does not throw away a cloud key.
#[tauri::command]
fn assistant_set_key(provider: assistant::Provider, key: Option<String>) -> Result<bool, String> {
    match key.map(|k| k.trim().to_string()) {
        Some(k) if !k.is_empty() => {
            secrets::store(provider.key_id(), &secrets::Secret::new(k)).map_err(|e| e.0)?;
            Ok(true)
        }
        _ => {
            secrets::delete(provider.key_id()).map_err(|e| e.0)?;
            Ok(false)
        }
    }
}

/// Remember that the assistant proposed this SQL.
///
/// Called when the user accepts a block into the editor — not when the model
/// writes one. A statement the assistant offered and you never used is not part
/// of your history, and should not be recorded as though it were.
#[tauri::command]
async fn remember_proposal(state: State<'_, AppState>, sql: String) -> Result<(), String> {
    state.proposals.lock().await.remember(&sql);
    Ok(())
}

/// Stream one reply.
///
/// Errors after the request has started are sent **as events**, not returned:
/// by then there may be half an answer on screen, and throwing it away to show
/// an error loses the more useful half.
// Eight parameters, and each is load-bearing: where to send the request, which
// model, the schema context, the conversation and the channel to stream back.
// Bundling them into a struct would only move the list.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn assistant_send(
    state: State<'_, AppState>,
    provider: assistant::Provider,
    base_url: String,
    model: String,
    connection_id: Option<String>,
    db: Option<String>,
    messages: Vec<assistant::ChatMessage>,
    on_event: tauri::ipc::Channel<assistant::StreamEvent>,
) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("No model is set. Choose one in Settings.".into());
    }
    let key = secrets::load(provider.key_id()).map_err(|e| e.0)?;
    if key.is_none() && provider.requires_key(&base_url) {
        return Err("No API key is set. Add one in Settings to use the assistant.".into());
    }

    // Schema, when there is a live connection to take it from. Names and types
    // only — `render_schema` cannot see row data, by construction.
    let mut caps: Option<crate::engine::Capabilities> = None;
    let (server_version, schema_text) = match &connection_id {
        Some(id) => match session::server(&state, id).await {
            Ok(server) => {
                let version = server.server_version.clone();
                caps = Some(server.capabilities.clone());

                // An engine with no namespaces — a local Elasticsearch cluster
                // has no catalogs — describes itself under the empty one. For
                // MySQL an empty schema name simply matches nothing, which is
                // the right answer when no database has been chosen.
                let ns = db.as_deref().unwrap_or("");

                // Load the shape *before* rendering it. The cache used to be
                // whatever the user had happened to expand in the tree, so the
                // model was handed "columns not loaded" and invented the rest.
                // A failure here is not fatal: an unreachable server should
                // produce a thinner prompt, not a refused question.
                let warmed =
                    schema::warm_for_assistant(&state, id, ns, schema::ASSISTANT_TABLE_BUDGET)
                        .await
                        .unwrap_or_default();

                let text = server
                    .schema_cache
                    .lock()
                    .await
                    .get(ns)
                    .map(|s| assistant::render_schema(ns, s))
                    .map(|rendered| {
                        // Say so when the budget bit. Silently truncating is
                        // how a model ends up confidently sure a table has no
                        // columns.
                        if warmed.tables > warmed.detailed {
                            format!(
                                "{rendered}\n(Columns were loaded for {} of {} tables; ask the \
                                 user to open the others in the tree if you need them.)\n",
                                warmed.detailed, warmed.tables
                            )
                        } else {
                            rendered
                        }
                    });
                (version, text)
            }
            Err(_) => ("unknown".to_string(), None),
        },
        None => ("unknown".to_string(), None),
    };

    let system = assistant::system_prompt(&server_version, schema_text.as_deref(), caps.as_ref());
    let body = assistant::request(provider, &model, &system, &messages);

    // Auth differs by provider, and a local server usually wants none at all —
    // so the header is attached only when there is a key to attach.
    let mut request = reqwest::Client::new()
        .post(provider.endpoint(&base_url))
        .header("content-type", "application/json");
    if let Some(key) = &key {
        request = match provider {
            assistant::Provider::Anthropic => request
                .header("x-api-key", key.expose())
                .header("anthropic-version", assistant::API_VERSION),
            assistant::Provider::OpenAiCompatible => {
                request.header("authorization", format!("Bearer {}", key.expose()))
            }
        };
    }

    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Could not reach {}: {e}", base_url.trim()))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        // Returned rather than sent as an event: nothing has been shown yet.
        return Err(assistant::http_error(status, &text));
    }

    let mut response = response;
    let mut decoder = assistant::SseDecoder::new(provider);
    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => {
                // Lossy on purpose: a multi-byte character split across chunks
                // must not abort a reply. The decoder buffers whole events, so
                // the only cost is a replacement char in the rare split case.
                let chunk = String::from_utf8_lossy(&bytes);
                for event in decoder.push(&chunk) {
                    let _ = on_event.send(event);
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = on_event.send(assistant::StreamEvent::Failed {
                    message: format!("The reply was cut off: {e}"),
                });
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Newest first, deduplicated by statement, optionally filtered.
#[tauri::command]
fn history_search(
    app: tauri::AppHandle,
    query: Option<String>,
    connection_id: Option<String>,
    limit: usize,
) -> Result<Vec<history::Hit>, String> {
    let dir = config_dir(&app)?;
    Ok(history::search(
        &dir,
        query.as_deref(),
        connection_id.as_deref(),
        limit.clamp(1, 1000),
    ))
}

/// Forget everything, or everything for one connection. Only ever called after
/// the user says yes.
#[tauri::command]
fn history_clear(app: tauri::AppHandle, connection_id: Option<String>) -> Result<(), String> {
    history::clear(&config_dir(&app)?, connection_id.as_deref())
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
        // ones need a connection. `Dialect::default()` is MySQL, which is also
        // what the editor highlights an unattached tab as — the two fall back
        // together rather than each guessing.
        return Ok(lint::lint(
            &sql,
            &lint::LintSchema::new(),
            lint::Dialect::default(),
        ));
    };
    // From the engine, not from its name: a driver that quotes identifiers with
    // `"` had every identifier masked away and was silently linted as an empty
    // statement.
    let engine = &tab.server.engine;
    let dialect = lint::Dialect {
        ident_quote: engine.ident_quote(),
        delimiter_blocks: engine.capabilities().delimiter_blocks,
    };
    let db = tab.current_db.lock().await.clone();
    let schema = schema::lint_schema(&tab.server, db.as_deref()).await;
    Ok(lint::lint(&sql, &schema, dialect))
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

#[tauri::command]
async fn list_routines(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
) -> Result<Vec<RoutineRef>, String> {
    schema::list_routines(&state, &connection_id, &db).await
}

/// The re-runnable creation script for a routine. Text only — like everything
/// in Stage 3, it lands in an editor tab and is never executed here.
#[tauri::command]
async fn routine_ddl(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
    name: String,
    kind: RoutineKind,
) -> Result<String, String> {
    schema::routine_ddl(&state, &connection_id, &db, &name, kind).await
}

/// The server's own `CREATE` for a table or a view. Text only, like
/// [`routine_ddl`].
///
/// One command for both because `SHOW CREATE TABLE` is what MySQL answers a
/// view with too — it simply names the column "Create View". Splitting them in
/// the UI would mean the caller had to know which it was holding, and the tree
/// already does; splitting them here would buy nothing.
#[tauri::command]
async fn table_ddl(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
    table: String,
) -> Result<String, String> {
    schema::table_ddl(&state, &connection_id, &db, &table).await
}

// ------------------------------------------------------------- generated SQL
//
// Every command here returns **text for the user to read**. None of them
// executes anything: that is the rule Stage 3 is built on, and half of what
// they produce is `DROP`.

/// Numbers the UI would otherwise have to hardcode a second time.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppDefaults {
    /// Rows a generated `SELECT` asks for.
    browse_limit: u32,
    /// The safety ceiling the executor applies per statement. A different
    /// concept from the browse limit, shown so the UI can explain truncation.
    max_rows: u32,
}

#[tauri::command]
fn app_defaults() -> AppDefaults {
    AppDefaults {
        browse_limit: sqlgen::DEFAULT_BROWSE_LIMIT,
        max_rows: exec::MAX_ROWS as u32,
    }
}

/// Browse a table — the double-click action.
///
/// Takes a connection now, because the snippet is dialect-specific: a
/// backtick-quoted, catalog-qualified `SELECT` is valid MySQL and a syntax
/// error on Elasticsearch. The engine decides; this only asks.
#[tauri::command]
async fn generate_select(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
    table: String,
    limit: u32,
) -> Result<String, String> {
    let server = session::server(&state, &connection_id).await?;
    server.engine.select_snippet(&db, &table, limit)
}

#[tauri::command]
fn generate_drop(target: sqlgen::DropTarget) -> Result<String, String> {
    sqlgen::generate_drop(&target)
}

#[tauri::command]
async fn generate_call(
    state: State<'_, AppState>,
    connection_id: String,
    db: String,
    name: String,
) -> Result<String, String> {
    // Resolved from the cached routine list rather than taken from the caller:
    // the parameter list is what makes the snippet useful, and the frontend
    // should not be the thing that remembers it.
    let routines = schema::list_routines(&state, &connection_id, &db).await?;
    let routine = routines
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| format!("No routine named {name} in {db}."))?;
    sqlgen::generate_call(&db, &routine)
}

// ------------------------------------------------------------------- export

/// Ask for a destination, returning `None` when the user cancels.
///
/// File I/O stays in Rust, as it has since Stage 1 — `tauri-plugin-fs` exists to
/// hand the filesystem to JavaScript, and we have never needed that.
async fn pick_export_path(
    app: &tauri::AppHandle,
    suggested_name: &str,
    label: &str,
    extensions: &[&str],
) -> Result<Option<std::path::PathBuf>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(suggested_name)
        .add_filter(label, extensions)
        .save_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx
        .await
        .map_err(|_| "The file dialog closed unexpectedly.".to_string())?;
    let Some(f) = picked else { return Ok(None) };
    f.into_path()
        .map(Some)
        .map_err(|e| format!("Unsupported file path: {e}"))
}

#[tauri::command]
async fn export_csv(
    app: tauri::AppHandle,
    result: export::ResultSet,
    options: export::CsvOptions,
    suggested_name: String,
) -> Result<Option<export::ExportOutcome>, String> {
    let export::ResultSet {
        columns,
        rows,
        truncated,
    } = result;
    let Some(path) = pick_export_path(&app, &suggested_name, "CSV", &["csv"]).await? else {
        return Ok(None);
    };
    let body = export::to_csv(&columns, &rows, &options);
    let bytes_written = export::write(&path, &body)?;

    let mut warnings = Vec::new();
    if let Some(w) = export::binary_column_warning(&columns) {
        warnings.push(w);
    }
    // CSV has no way to spell NULL. Whichever way the collapse goes, say it.
    if options.null_as.is_empty() && rows.iter().flatten().any(|c| matches!(c, CellValue::Null)) {
        warnings.push("NULLs were written as empty fields; CSV cannot tell the two apart.".into());
    }

    Ok(Some(export::ExportOutcome {
        path: path.display().to_string(),
        rows_written: rows.len(),
        truncated_source: truncated,
        bytes_written,
        warnings,
    }))
}

/// Export rows as an `INSERT` script.
///
/// When `source` names a real table, its `CREATE TABLE` comes from the server
/// and is exact. Otherwise it is derived from the result's column metadata and
/// says so in the file, because the protocol carries no lengths or keys.
#[tauri::command]
async fn export_inserts(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    result: export::ResultSet,
    options: export::InsertOptions,
    source: Option<SourceTable>,
    suggested_name: String,
) -> Result<Option<export::ExportOutcome>, String> {
    let export::ResultSet {
        columns,
        rows,
        truncated,
    } = result;
    let mut warnings = Vec::new();

    let create_sql = if options.create_table {
        match &source {
            Some(src) => {
                Some(schema::table_ddl(&state, &src.connection_id, &src.db, &src.table).await?)
            }
            None => {
                warnings.push(
                    "These rows did not come from a single table, so the CREATE TABLE was \
                     derived from the result's columns: types are widened and there are no \
                     keys or defaults."
                        .into(),
                );
                Some(export::derived_create_table(
                    &columns,
                    options.db.as_deref(),
                    &options.table,
                )?)
            }
        }
    } else {
        None
    };

    // Generate before opening the dialog: a refusal (a binary column, say) must
    // not arrive after the user has already chosen a filename.
    let body = export::to_inserts(&columns, &rows, &options, create_sql.as_deref())?;

    let Some(path) = pick_export_path(&app, &suggested_name, "SQL script", &["sql"]).await? else {
        return Ok(None);
    };
    let bytes_written = export::write(&path, &body)?;

    Ok(Some(export::ExportOutcome {
        path: path.display().to_string(),
        rows_written: rows.len(),
        truncated_source: truncated,
        bytes_written,
        warnings,
    }))
}

/// Re-run a statement with no row ceiling and stream the result to a file.
///
/// The honest answer to "export a truncated result": the rows on screen are
/// capped by `MAX_ROWS`, and this goes back to the server for all of them. Only
/// offered when the statement can be re-run standalone — see
/// `export::rerunnable`, which refuses rather than guessing.
#[tauri::command]
async fn export_rerun(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    tab_id: String,
    sql: String,
    format: export::ExportFormat,
    options: export::ExportOptions,
    suggested_name: String,
) -> Result<Option<export::ExportOutcome>, String> {
    // Checked before the dialog: a refusal must not arrive after the user has
    // already chosen a filename.
    let statement = export::rerunnable(&sql)?;

    let (label, ext) = match format {
        export::ExportFormat::Csv => ("CSV", "csv"),
        export::ExportFormat::Inserts => ("SQL script", "sql"),
    };
    let Some(path) = pick_export_path(&app, &suggested_name, label, &[ext]).await? else {
        return Ok(None);
    };

    let (rows_written, bytes_written, cancelled) = match format {
        export::ExportFormat::Csv => {
            let o = options.csv.clone();
            let h = options.csv.clone();
            exec::stream_to_file(
                &state,
                &tab_id,
                &statement,
                &path,
                move |row, buf| {
                    buf.push_str(&export::csv_row(row, &o));
                    Ok(())
                },
                move |row| Ok(export::csv_header(row, &h)),
            )
            .await?
        }
        export::ExportFormat::Inserts => {
            let o = options.inserts.clone();
            let h = options.inserts.clone();
            exec::stream_to_file(
                &state,
                &tab_id,
                &statement,
                &path,
                move |row, buf| export::insert_row(row, &o, buf),
                move |row| export::insert_header(row, &h),
            )
            .await?
        }
    };

    let mut warnings = Vec::new();
    if cancelled {
        warnings.push(format!(
            "Cancelled after {rows_written} rows. The file holds what had been read by then."
        ));
    }

    Ok(Some(export::ExportOutcome {
        path: path.display().to_string(),
        rows_written,
        // This path went back to the server for everything, so by construction
        // it is not working from a truncated set.
        truncated_source: false,
        bytes_written,
        warnings,
    }))
}

/// The table a result set came from, when it came from one.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceTable {
    connection_id: String,
    db: String,
    table: String,
}

/// Text for the clipboard. Returns the string rather than writing it, so the
/// frontend owns the one clipboard call and we do not need a second mechanism.
#[tauri::command]
fn clipboard_text(result: export::ResultSet, options: export::CsvOptions) -> String {
    let export::ResultSet { columns, rows, .. } = result;
    // No BOM and no CRLF: this is going into another program's buffer, not a
    // file Excel will open cold.
    let opts = export::CsvOptions {
        bom: false,
        crlf: false,
        ..options
    };
    export::to_csv(&columns, &rows, &opts)
}

// ------------------------------------------------------------------- stateless

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

/// Choose the TLS backend, once, before anything opens a connection.
///
/// **This is not optional, and its absence was a latent crash.** `reqwest` is
/// built with `rustls-no-provider` — chosen so that `aws-lc-rs`, which needs
/// cmake and NASM, stays out of the Windows build — and that feature makes the
/// caller responsible for installing a crypto provider. Nothing did, so the
/// first `Client::new()` would have panicked: the assistant's first request on
/// Linux, where the updater never builds a client because it refuses earlier.
///
/// Found by a unit test in `httpsql`, not by using the app.
///
/// Idempotent by design: `install_default` returns `Err` if a provider is
/// already set, which is a fact rather than a failure.
///
/// Public because integration tests build their own `reqwest` clients without
/// ever calling [`run`], so they hit the same panic. Sharing the one function
/// keeps the test's TLS setup identical to the app's rather than merely similar.
pub fn install_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// The licence notices this build is obliged to carry.
///
/// Generated per target at package time by `scripts/attribution.mjs` and shipped
/// as a bundle resource, because a file that lives only in the repository
/// discharges nothing for someone who installed the `.deb`.
///
/// A build run straight from a source tree may not have one — the generator is
/// part of packaging, not of `cargo run` — so the absence says what to do rather
/// than reading as a failure.
#[tauri::command]
fn third_party_licenses(app: tauri::AppHandle) -> Result<String, String> {
    let path = app
        .path()
        .resolve(LICENSES_FILE, tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("Cannot locate {LICENSES_FILE}: {e}"))?;

    std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "This build carries no licence file. It is generated when the app is              packaged; from a source tree, run:\n\n    npm run attribution\n\n             Expected at {}",
            path.display()
        )
    })
}

/// Ask the update endpoint whether there is anything newer.
///
/// Errors are *returned*, never thrown away: a check that fails silently is
/// indistinguishable from a check that found nothing, and the difference
/// matters when someone is waiting for a fix.
#[tauri::command]
async fn update_check(app: tauri::AppHandle) -> Result<update::UpdateStatus, String> {
    let current = app.package_info().version.to_string();
    if !update::supported() {
        return Ok(update::UpdateStatus::Unsupported {
            reason: update::UNSUPPORTED_REASON.into(),
        });
    }
    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_updater::UpdaterExt;
        let updater = app.updater().map_err(|e| e.to_string())?;
        return match updater.check().await.map_err(|e| e.to_string())? {
            Some(u) => Ok(update::UpdateStatus::Available {
                current,
                version: u.version.clone(),
                notes: u.body.clone(),
                date: u.date.map(|d| d.to_string()),
            }),
            None => Ok(update::UpdateStatus::UpToDate { current }),
        };
    }
    #[cfg(not(target_os = "windows"))]
    Ok(update::UpdateStatus::UpToDate { current })
}

/// Download and apply the update. **Only ever called after the user agreed.**
///
/// Re-checks rather than holding the handle from `update_check` across the
/// dialog. One extra request buys no cross-command state to keep in sync and
/// no chance of applying a stale result; and if a newer release appeared while
/// the dialog was open, installing *that* is the right answer anyway.
///
/// Windows exits the app to run the installer, so this may never return.
#[tauri::command]
async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    if !update::supported() {
        return Err(update::UNSUPPORTED_REASON.into());
    }
    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_updater::UpdaterExt;
        let updater = app.updater().map_err(|e| e.to_string())?;
        let Some(u) = updater.check().await.map_err(|e| e.to_string())? else {
            return Err("There is no update to install.".into());
        };
        u.download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Err(update::UNSUPPORTED_REASON.into())
    }
}

// -------------------------------------------------------------------- MCP server
//
// Everything here is off until the user turns it on. See `mcp.rs` for why a
// listening socket is acceptable in an app whose oldest rule is that nothing
// runs without the person at the keyboard pressing run.

/// Start or restart the MCP server on `port`.
///
/// Fails loudly on a busy port rather than logging: the switch in Settings has
/// to be able to say why it did not turn on.
#[tauri::command]
async fn mcp_start(
    app: tauri::AppHandle,
    state: State<'_, mcp::McpState>,
    port: u16,
) -> Result<mcp::McpStatus, String> {
    mcp::start(&app, &state, port).await
}

#[tauri::command]
async fn mcp_stop(state: State<'_, mcp::McpState>) -> Result<mcp::McpStatus, String> {
    mcp::stop(&state).await;
    Ok(state.status().await)
}

#[tauri::command]
async fn mcp_status(state: State<'_, mcp::McpState>) -> Result<mcp::McpStatus, String> {
    Ok(state.status().await)
}

/// Which connection an MCP client sees. Pushed by the frontend when the user
/// switches, because "the connection you are looking at" is a fact only the UI
/// holds — see the `Focus` docs for why no other command works this way.
#[tauri::command]
async fn mcp_set_focus(
    state: State<'_, mcp::McpState>,
    connection_id: Option<String>,
) -> Result<(), String> {
    state.set_focus(connection_id).await;
    Ok(())
}

/// The bearer token, minted on first read. Returned so Settings can show it
/// and copy it into a client's config — it is a credential for this machine's
/// own loopback port, and the user is the only one who can put it anywhere.
#[tauri::command]
fn mcp_token() -> Result<String, String> {
    mcp::token()
}

#[tauri::command]
fn mcp_regenerate_token() -> Result<String, String> {
    mcp::regenerate()
}

pub fn run() {
    // Before any client is built, by anything.
    install_tls();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .manage(mcp::McpState::default())
        .invoke_handler(tauri::generate_handler![
            connect,
            connect_saved,
            disconnect,
            list_connections,
            list_profiles,
            save_profile,
            delete_profile,
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
            list_routines,
            routine_ddl,
            table_ddl,
            third_party_licenses,
            assistant_status,
            assistant_set_key,
            assistant_send,
            remember_proposal,
            history_search,
            history_clear,
            load_session,
            save_session,
            app_defaults,
            generate_select,
            generate_drop,
            generate_call,
            export_csv,
            export_inserts,
            clipboard_text,
            export_rerun,
            statement_at_cursor,
            supported_file_types,
            open_file_dialog,
            read_file,
            save_file,
            save_file_dialog,
            update_check,
            update_install,
            mcp_start,
            mcp_stop,
            mcp_status,
            mcp_set_focus,
            mcp_token,
            mcp_regenerate_token,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
