//! Connection registry, per-connection servers, and per-tab sessions.
//!
//! # Model (Stage 2)
//!
//! Several servers can be live at once. Each live connection owns:
//!
//!   * one **exec** connection per tab, opened lazily on that tab's first run;
//!   * one shared **killer**, which issues `KILL QUERY` and nothing else;
//!   * one shared **meta**, for `information_schema` introspection.
//!
//! So the budget is `(tabs + 2) × live connections`, with idle tabs free.
//!
//! # There is no "active connection" here
//!
//! Which connection the user is looking at is a frontend concern. Every command
//! names the connection or tab it acts on, and a `TabSession` holds an
//! `Arc<ServerConn>` so a tab physically cannot run against the wrong server.
//! An ambient "current connection" in the backend is what would let a UI switch
//! race an in-flight query onto the wrong machine.
//!
//! # Lock discipline
//!
//! Stage 0 shipped a deadlock from one over-broad mutex; Stage 1 scaled the
//! same trap up to a tab map. The rules, in order of how badly they bite:
//!
//! 1. **Never hold the `connections` or `tabs` map lock across an `await`.**
//!    Clone the `Arc` out, drop the guard, then do the work.
//! 2. **`cancel_query` touches only its connection's `killer` and the tab's
//!    `conn_id`.** It must never take that tab's `exec` lock — the running
//!    query holds it.
//! 3. **The killer connection does nothing else.** If it can be busy, cancel
//!    can be blocked.
//! 4. Lock order: `tabs` → `TabSession.exec` → `ServerConn.*`.
//! 5. **`disconnect(id)` closes that connection's tabs and only that
//!    connection's**, matched by `Arc::ptr_eq`, never by a stale copy of an id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
use sqlx::{Connection, MySqlConnection, Row};
use tokio::sync::Mutex;

use crate::schema::DbSchema;

/// Opaque, minted by the frontend. Rust only ever maps it to a session.
pub type TabId = String;
/// Opaque, minted by the frontend. Also the keychain account name in Phase 2.
pub type ConnectionId = String;

/// Everything about a connection except its secret.
///
/// Deliberately holds no password: this is the shape Phase 2 will persist to
/// disk, so keeping it secret-free by construction means the config file cannot
/// leak one even by accident.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnProfile {
    pub id: ConnectionId,
    pub name: String,
    /// Identification, not decoration — answers "which server am I on".
    #[serde(default = "default_colour")]
    pub colour: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub database: Option<String>,
    #[serde(default)]
    pub allow_invalid_certs: bool,

    /// Which engine this connects to.
    ///
    /// `#[serde(default)]` throughout, so every `connections.json` written
    /// before Stage 11 loads unchanged and means MySQL — the same
    /// backward-compatible move `workspace.rs` relies on.
    #[serde(default)]
    pub kind: EngineKind,
    /// Base URL, for engines addressed by one. Ignored by MySQL, which uses
    /// `host`/`port`.
    #[serde(default)]
    pub url: String,
    /// How to authenticate an HTTP engine. MySQL always uses user + password.
    #[serde(default)]
    pub auth: crate::httpsql::Auth,
}

/// Which engine a profile names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineKind {
    /// The default, so a profile written before engines existed is MySQL.
    #[default]
    Mysql,
    Elasticsearch,
}

/// A profile as the **UI** sees it: the persisted fields, plus the facts that
/// are derived at runtime rather than stored.
///
/// This exists because `ConnProfile` has two destinations with opposite needs.
/// The config file must not claim a password that a copy of the file cannot
/// have; the frontend must be told when one exists, or it will prompt for a
/// password it already has. A field on `ConnProfile` cannot satisfy both — it
/// was `#[serde(skip)]` for the file's sake, which silently stripped it from
/// the IPC response too and made every remembered password invisible at boot.
///
/// So the derived fact lives on the wire type only, and `ConnProfile` stays
/// exactly what gets written to disk. `flatten` keeps the JSON a flat object,
/// which is what the frontend's `ConnProfile` already expects.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    #[serde(flatten)]
    pub profile: ConnProfile,
    /// True when a secret exists in the keychain for this id. Never persisted,
    /// so a profile copied to another machine reports the truth there.
    pub remember_password: bool,
}

impl ProfileView {
    pub fn new(profile: ConnProfile, remember_password: bool) -> Self {
        Self {
            profile,
            remember_password,
        }
    }
}

fn default_port() -> u16 {
    3306
}
fn default_colour() -> String {
    "#3b82f6".into()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnInfo {
    pub id: ConnectionId,
    pub server_version: String,
    pub databases: Vec<String>,
    pub current_database: Option<String>,
    /// What this engine supports. Sent to the frontend because a menu offering
    /// `DROP` against a read-only engine is a bug the backend cannot prevent.
    pub capabilities: crate::engine::Capabilities,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabStatus {
    pub connected: bool,
    pub running: bool,
    pub current_database: Option<String>,
    /// 0 until this tab has opened its exec connection.
    pub connection_id: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub id: ConnectionId,
    pub name: String,
    pub colour: String,
    pub host_label: String,
    pub connected: bool,
    pub server_version: Option<String>,
    pub open_tabs: usize,
    pub running_tabs: usize,
}

/// One live server: its identity, the two shared connections, and the schema
/// cache (a property of the server, not of any tab).
pub struct ServerConn {
    pub profile: ConnProfile,
    /// Held for the connection's life because lazy per-tab connections must be
    /// able to authenticate later. Phase 2 replaces this with a zeroizing
    /// `Secret`; it is never serialised or logged.
    password: String,
    /// The two shared MySQL connections. `None` for engines that hold nothing
    /// open — HTTP is stateless, so there is nothing to keep or to leak.
    pub killer: Option<Mutex<MySqlConnection>>,
    pub meta: Option<Mutex<MySqlConnection>>,
    pub schema_cache: Mutex<HashMap<String, DbSchema>>,
    /// Everything that differs between engines. MySQL's implementation is a
    /// dispatch vtable over the code that has always been here; see `mysql.rs`.
    pub engine: Box<dyn crate::engine::Engine>,
    pub server_version: String,
    /// What this connection's engine supports. Held here so `exec` can ask
    /// without knowing which engine it is talking to.
    pub capabilities: crate::engine::Capabilities,
    /// Counts information_schema round trips; the cache's job is to keep this
    /// flat on repeat reads, which is what tests assert on.
    pub introspection_count: AtomicU64,
}

impl ServerConn {
    /// The shared MySQL connections, for the code that is MySQL's.
    ///
    /// A `Result` rather than a panic: reaching these on a connection that has
    /// none means a MySQL-only path was called for another engine, which is a
    /// bug worth a message rather than a crash in someone's session.
    pub(crate) fn mysql_meta(&self) -> Result<&Mutex<MySqlConnection>, String> {
        self.meta
            .as_ref()
            .ok_or_else(|| "This operation needs a MySQL connection.".to_string())
    }

    pub(crate) fn mysql_killer(&self) -> Result<&Mutex<MySqlConnection>, String> {
        self.killer
            .as_ref()
            .ok_or_else(|| "This operation needs a MySQL connection.".to_string())
    }

    pub fn id(&self) -> &str {
        &self.profile.id
    }

    pub fn host_label(&self) -> String {
        format!(
            "{}@{}:{}",
            self.profile.user, self.profile.host, self.profile.port
        )
    }
}

/// Per-tab state. Everything a script needs to run independently of its
/// siblings — including which server it belongs to.
pub struct TabSession {
    /// Binds the tab to one connection for its whole life.
    pub server: Arc<ServerConn>,
    pub exec: Mutex<Option<MySqlConnection>>,
    pub conn_id: AtomicU64,
    pub current_db: Mutex<Option<String>>,
    pub running: AtomicBool,
    pub cancel_requested: AtomicBool,
}

impl TabSession {
    fn new(server: Arc<ServerConn>) -> Self {
        Self {
            server,
            exec: Mutex::new(None),
            conn_id: AtomicU64::new(0),
            current_db: Mutex::new(None),
            running: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
        }
    }
}

#[derive(Default)]
pub struct AppState {
    pub connections: Mutex<HashMap<ConnectionId, Arc<ServerConn>>>,
    /// Flat, because a tab already knows its server.
    pub tabs: Mutex<HashMap<TabId, Arc<TabSession>>>,
    /// SQL the assistant proposed this session, so history can say where a
    /// statement came from. In memory only: provenance is a fact about this
    /// run, and a stale proposal file would start mislabelling things.
    pub proposals: Mutex<crate::assistant::Proposals>,
}

/// sqlx errors are verbose and full of internals. Surface the part a human
/// can act on — and never anything derived from the password.
pub fn friendly(e: &sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(db) => match db.code().as_deref() {
            Some("1045") => "Access denied — check the username and password.".into(),
            Some("1049") => format!("Unknown database: {}", db.message()),
            Some("2005") => "Unknown host — check the hostname.".into(),
            _ => db.message().to_string(),
        },
        sqlx::Error::Io(io) => format!("Cannot reach the server: {io}"),
        sqlx::Error::Tls(t) => {
            format!("TLS handshake failed: {t}. If this is an internal server with a self-signed certificate, enable \"Allow invalid certificates\".")
        }
        sqlx::Error::PoolTimedOut => "Timed out connecting to the server.".into(),
        other => other.to_string(),
    }
}

fn options(profile: &ConnProfile, password: &str) -> MySqlConnectOptions {
    let mut o = MySqlConnectOptions::new()
        .host(&profile.host)
        .port(profile.port)
        .username(&profile.user)
        .password(password)
        // Secure by default: verify the CA chain and the hostname. The toggle
        // downgrades to "encrypted but unverified", which is what an internal
        // server with a self-signed cert needs — it never falls back to
        // plaintext.
        .ssl_mode(if profile.allow_invalid_certs {
            MySqlSslMode::Required
        } else {
            MySqlSslMode::VerifyIdentity
        });
    if let Some(db) = profile.database.as_deref().filter(|d| !d.is_empty()) {
        o = o.database(db);
    }
    o
}

async fn open(profile: &ConnProfile, password: &str) -> Result<MySqlConnection, String> {
    MySqlConnection::connect_with(&options(profile, password))
        .await
        .map_err(|e| friendly(&e))
}

// ------------------------------------------------------------------ lookups

/// A live connection by id. Clones the `Arc` out and releases the map lock
/// immediately — see rule 1.
pub async fn server(state: &AppState, id: &str) -> Result<Arc<ServerConn>, String> {
    state
        .connections
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| "That connection is not open.".to_string())
}

/// A tab's session. Errors if the tab was never attached to a connection —
/// which is a bug in the caller, not something to paper over by guessing.
pub async fn tab(state: &AppState, id: &str) -> Result<Arc<TabSession>, String> {
    state
        .tabs
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| "This tab is not attached to a connection.".to_string())
}

pub async fn tab_if_open(state: &AppState, id: &str) -> Option<Arc<TabSession>> {
    state.tabs.lock().await.get(id).cloned()
}

// ------------------------------------------------------------- connect/close

pub async fn connect(
    state: &AppState,
    profile: ConnProfile,
    password: String,
) -> Result<ConnInfo, String> {
    // Reconnecting an id replaces the old session rather than stacking one.
    disconnect(state, &profile.id).await.ok();

    if profile.kind == EngineKind::Elasticsearch {
        return connect_elastic(state, profile, password).await;
    }

    // Only the two shared connections up front. Tab connections are lazy, so
    // connecting costs 2 regardless of how many tabs the connection has.
    let mut meta = open(&profile, &password).await?;
    let killer = open(&profile, &password).await?;

    let server_version: String = sqlx::query("SELECT VERSION() AS v")
        .fetch_one(&mut meta)
        .await
        .map_err(|e| friendly(&e))?
        .try_get("v")
        .map_err(|e| friendly(&e))?;

    let databases: Vec<String> = sqlx::query(
        "SELECT schema_name AS name FROM information_schema.schemata ORDER BY schema_name",
    )
    .fetch_all(&mut meta)
    .await
    .map_err(|e| friendly(&e))?
    .into_iter()
    .filter_map(|r| r.try_get::<String, _>("name").ok())
    .collect();

    let id = profile.id.clone();
    let current_database = profile.database.clone().filter(|d| !d.is_empty());

    let conn = Arc::new(ServerConn {
        profile,
        password,
        killer: Some(Mutex::new(killer)),
        meta: Some(Mutex::new(meta)),
        schema_cache: Mutex::new(HashMap::new()),
        engine: Box::new(crate::mysql::MysqlEngine::new()),
        server_version: server_version.clone(),
        capabilities: crate::engine::Capabilities::mysql(),
        introspection_count: AtomicU64::new(0),
    });
    state.connections.lock().await.insert(id.clone(), conn);

    Ok(ConnInfo {
        id,
        server_version,
        databases,
        current_database,
        capabilities: crate::engine::Capabilities::mysql(),
    })
}

/// Close one connection: its tabs' exec connections, then its shared pair.
/// Other connections are untouched.
pub async fn disconnect(state: &AppState, id: &str) -> Result<(), String> {
    let Some(server) = state.connections.lock().await.remove(id) else {
        return Ok(());
    };

    // Take this connection's tabs out of the map in one pass, matched by
    // identity rather than by an id that could have been reused.
    let doomed: Vec<Arc<TabSession>> = {
        let mut guard = state.tabs.lock().await;
        let ids: Vec<TabId> = guard
            .iter()
            .filter(|(_, t)| Arc::ptr_eq(&t.server, &server))
            .map(|(k, _)| k.clone())
            .collect();
        ids.iter().filter_map(|k| guard.remove(k)).collect()
    };

    for t in doomed {
        // A running query holds the exec lock; KILL it first or closing the
        // connection would block until a 30-second query finished.
        t.cancel_requested.store(true, Ordering::SeqCst);
        if t.running.load(Ordering::SeqCst) {
            kill_on(&server, t.conn_id.load(Ordering::SeqCst))
                .await
                .ok();
        }
        if let Some(c) = t.exec.lock().await.take() {
            c.close().await.ok();
        }
    }

    // If nobody else is mid-operation, say goodbye properly. Otherwise the last
    // Arc holder dropping it closes the sockets anyway — either way no leak.
    if let Ok(s) = Arc::try_unwrap(server) {
        if let Some(k) = s.killer {
            k.into_inner().close().await.ok();
        }
        if let Some(m) = s.meta {
            m.into_inner().close().await.ok();
        }
    }
    Ok(())
}

/// Close every connection. Used on shutdown.
pub async fn disconnect_all(state: &AppState) -> Result<(), String> {
    let ids: Vec<ConnectionId> = state.connections.lock().await.keys().cloned().collect();
    for id in ids {
        disconnect(state, &id).await.ok();
    }
    Ok(())
}

pub async fn list_connections(state: &AppState) -> Vec<ConnectionStatus> {
    let servers: Vec<Arc<ServerConn>> = state.connections.lock().await.values().cloned().collect();
    let tabs: Vec<Arc<TabSession>> = state.tabs.lock().await.values().cloned().collect();

    servers
        .into_iter()
        .map(|s| {
            let mine: Vec<&Arc<TabSession>> =
                tabs.iter().filter(|t| Arc::ptr_eq(&t.server, &s)).collect();
            ConnectionStatus {
                id: s.profile.id.clone(),
                name: s.profile.name.clone(),
                colour: s.profile.colour.clone(),
                host_label: s.host_label(),
                connected: true,
                server_version: Some(s.server_version.clone()),
                open_tabs: mine.len(),
                running_tabs: mine
                    .iter()
                    .filter(|t| t.running.load(Ordering::SeqCst))
                    .count(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- tab lifecycle

/// Attach a tab to a connection. Idempotent for the same pair; re-attaching a
/// tab to a different connection replaces its session.
pub async fn open_tab(state: &AppState, connection_id: &str, tab_id: &str) -> Result<(), String> {
    let server = server(state, connection_id).await?;
    let mut guard = state.tabs.lock().await;
    match guard.get(tab_id) {
        Some(existing) if Arc::ptr_eq(&existing.server, &server) => Ok(()),
        _ => {
            guard.insert(tab_id.to_string(), Arc::new(TabSession::new(server)));
            Ok(())
        }
    }
}

/// Close a tab and release its connection.
pub async fn close_tab(state: &AppState, id: &str) -> Result<(), String> {
    let Some(tab) = state.tabs.lock().await.remove(id) else {
        return Ok(());
    };
    tab.cancel_requested.store(true, Ordering::SeqCst);
    if tab.running.load(Ordering::SeqCst) {
        kill_on(&tab.server, tab.conn_id.load(Ordering::SeqCst))
            .await
            .ok();
    }
    if let Some(c) = tab.exec.lock().await.take() {
        c.close().await.ok();
    }
    Ok(())
}

pub async fn tab_status(state: &AppState, id: &str) -> Result<TabStatus, String> {
    let Some(tab) = tab_if_open(state, id).await else {
        return Ok(TabStatus {
            connected: false,
            running: false,
            current_database: None,
            connection_id: 0,
        });
    };
    // Bind first: a guard temporary inside the struct literal would outlive
    // the borrow of `tab`.
    let current_database = tab.current_db.lock().await.clone();
    Ok(TabStatus {
        connected: true,
        running: tab.running.load(Ordering::SeqCst),
        current_database,
        connection_id: tab.conn_id.load(Ordering::SeqCst),
    })
}

// ------------------------------------------------------------- exec connection

/// Ensure `guard` holds a live connection for this tab, opening or replacing it
/// as needed. Call with the tab's `exec` lock already held.
///
/// Handles the connection MySQL reaped while the tab sat idle: `wait_timeout`
/// defaults to 8 hours, so a tab left open overnight *will* find its connection
/// gone. Without this the tab would be broken until the app restarted.
///
/// A replacement connection is a fresh session, so the tab's `USE` is
/// re-applied. Temporary tables and session variables cannot be restored —
/// that is inherent to losing the connection, not something we can paper over.
pub async fn ensure_exec(
    guard: &mut Option<MySqlConnection>,
    tab: &TabSession,
) -> Result<(), String> {
    if let Some(c) = guard.as_mut() {
        if c.ping().await.is_ok() {
            return Ok(());
        }
        *guard = None;
        tab.conn_id.store(0, Ordering::SeqCst);
    }

    let want_db = tab.current_db.lock().await.clone();
    let mut profile = tab.server.profile.clone();
    if want_db.is_some() {
        profile.database = want_db;
    }

    let mut c = open(&profile, &tab.server.password).await?;
    let id: u64 = sqlx::query("SELECT CONNECTION_ID() AS id")
        .fetch_one(&mut c)
        .await
        .map_err(|e| friendly(&e))?
        .try_get("id")
        .map_err(|e| friendly(&e))?;

    tab.conn_id.store(id, Ordering::SeqCst);
    *guard = Some(c);
    Ok(())
}

pub(crate) async fn mysql_use_database(
    state: &AppState,
    tab_id: &str,
    db: &str,
) -> Result<(), String> {
    let tab = tab(state, tab_id).await?;
    let quoted = crate::sqlgen::quote_ident(db)?;

    let mut guard = tab.exec.lock().await;
    ensure_exec(&mut guard, &tab).await?;
    let conn = guard.as_mut().expect("ensure_exec guarantees a connection");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("USE {quoted}")))
        .execute(&mut *conn)
        .await
        .map_err(|e| friendly(&e))?;
    drop(guard);

    *tab.current_db.lock().await = Some(db.to_string());
    Ok(())
}

// ------------------------------------------------------------------ cancel

/// Issue `KILL QUERY <id>` from a specific connection's killer.
async fn kill_on(server: &ServerConn, conn_id: u64) -> Result<(), String> {
    if conn_id == 0 {
        return Ok(()); // this tab has never opened a connection
    }
    let mut killer = server.mysql_killer()?.lock().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("KILL QUERY {conn_id}")))
        .execute(&mut *killer)
        .await
        .map_err(|e| friendly(&e))?;
    Ok(())
}

/// Cancel the query running in one tab, and only that tab.
///
/// Takes that tab's connection's killer lock and the tab's `conn_id`. It never
/// takes the tab's `exec` lock — the query being killed is holding it.
pub(crate) async fn mysql_cancel_query(state: &AppState, tab_id: &str) -> Result<(), String> {
    let Some(tab) = tab_if_open(state, tab_id).await else {
        return Ok(());
    };
    // Flag first: even if KILL races the query finishing, the script stops.
    tab.cancel_requested.store(true, Ordering::SeqCst);
    kill_on(&tab.server, tab.conn_id.load(Ordering::SeqCst)).await
}

// ------------------------------------------------------------ engine dispatch

/// The databases on a MySQL server. Split out so `MysqlEngine` can offer it as
/// `namespaces` without `connect` and the engine trait duplicating the query.
pub(crate) async fn mysql_databases(server: &ServerConn) -> Result<Vec<String>, String> {
    let mut meta = server.mysql_meta()?.lock().await;
    let rows = sqlx::query(
        "SELECT schema_name AS name FROM information_schema.schemata ORDER BY schema_name",
    )
    .fetch_all(&mut *meta)
    .await
    .map_err(|e| friendly(&e))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("name").ok())
        .collect())
}

/// Switch the namespace this tab works in — a database, a catalog, a schema.
pub async fn use_database(state: &AppState, tab_id: &str, db: &str) -> Result<(), String> {
    let tab = tab(state, tab_id).await?;
    let server = Arc::clone(&tab.server);
    server.engine.use_namespace(state, tab_id, db).await
}

/// Stop whatever this tab is running.
pub async fn cancel_query(state: &AppState, tab_id: &str) -> Result<(), String> {
    let tab = tab(state, tab_id).await?;
    let server = Arc::clone(&tab.server);
    server.engine.cancel(state, tab_id).await
}

/// Open a connection to an Elasticsearch cluster.
///
/// Shaped like the MySQL path on purpose — same `ConnInfo` out, same
/// `ServerConn` in the map — so everything above `connect` is unchanged.
///
/// The two shared MySQL connections have no counterpart here: HTTP is
/// stateless, so there is nothing to hold open and nothing to leak. That is why
/// `ServerConn` keeps them behind `Option` rather than this path inventing a
/// pair it would never use.
async fn connect_elastic(
    state: &AppState,
    profile: ConnProfile,
    password: String,
) -> Result<ConnInfo, String> {
    let secret = (!password.is_empty()).then_some(password);
    let engine = crate::elastic::ElasticEngine::new(&profile.url, profile.auth.clone(), secret);

    // Reachability and version in one call, before anything is stored: a
    // connection that failed must leave no trace, exactly as for MySQL.
    let server_version = engine.version().await?;
    let capabilities = engine.capabilities().clone();

    let id = profile.id.clone();
    let current_database = profile.database.clone().filter(|d| !d.is_empty());

    let conn = Arc::new(ServerConn {
        profile,
        password: String::new(),
        killer: None,
        meta: None,
        schema_cache: Mutex::new(HashMap::new()),
        engine: Box::new(engine),
        server_version: server_version.clone(),
        capabilities: capabilities.clone(),
        introspection_count: AtomicU64::new(0),
    });

    // Catalogs are this engine's namespaces; a cluster with cross-cluster
    // search off simply has none, which the tree renders as no children.
    let databases = conn.engine.namespaces(&conn).await.unwrap_or_default();
    state.connections.lock().await.insert(id.clone(), conn);

    Ok(ConnInfo {
        id,
        server_version,
        databases,
        current_database,
        capabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConnProfile {
        ConnProfile {
            id: "c1-abc".into(),
            name: "prod-eu".into(),
            colour: "#ef4444".into(),
            host: "db.example.com".into(),
            port: 3306,
            user: "reporting".into(),
            database: Some("analytics".into()),
            allow_invalid_certs: false,
            kind: Default::default(),
            url: String::new(),
            auth: Default::default(),
        }
    }

    /// The regression test for the bug where every remembered password looked
    /// forgotten after a restart.
    ///
    /// `remember_password` used to be a `#[serde(skip)]` field on
    /// `ConnProfile`, which is right for the config file and wrong for the IPC
    /// response — and the same derive served both. The backend dutifully set
    /// the flag from the keychain and serde dropped it on the way out, so the
    /// UI saw `undefined`, decided nothing was stored, and prompted.
    ///
    /// Nothing failed and nothing logged. Only the wire format could have shown
    /// it, so that is what this asserts.
    #[test]
    fn the_ui_is_told_when_a_password_is_remembered() {
        let json = serde_json::to_value(ProfileView::new(sample(), true)).unwrap();
        assert_eq!(
            json["rememberPassword"], true,
            "the frontend cannot use a stored password it is never told about: {json}"
        );
        assert_eq!(json["rememberPassword"], serde_json::json!(true));
    }

    #[test]
    fn a_profile_with_no_stored_password_says_so() {
        let json = serde_json::to_value(ProfileView::new(sample(), false)).unwrap();
        assert_eq!(json["rememberPassword"], false);
    }

    /// `flatten` must produce one flat object, not a nested `profile` key —
    /// the frontend's `ConnProfile` reads `id`/`host` at the top level.
    #[test]
    fn the_view_is_flat_and_camel_cased() {
        let json = serde_json::to_value(ProfileView::new(sample(), true)).unwrap();
        assert!(
            json.get("profile").is_none(),
            "nested, not flattened: {json}"
        );
        assert_eq!(json["id"], "c1-abc");
        assert_eq!(json["host"], "db.example.com");
        assert_eq!(json["allowInvalidCerts"], false);
    }

    /// The other half of the invariant: the flag is derived, so it must not be
    /// something a config file — or a frontend — can assert. `ConnProfile` has
    /// no field for it at all, which is why round-tripping cannot invent one.
    #[test]
    fn a_profile_cannot_claim_a_password_it_does_not_have() {
        let raw = r##"{"id":"c1-abc","name":"n","colour":"#000000","host":"h","port":3306,
                       "user":"u","database":null,"allowInvalidCerts":false,
                       "rememberPassword":true}"##;
        let p: ConnProfile = serde_json::from_str(raw).unwrap();
        let json = serde_json::to_value(&p).unwrap();
        assert!(
            json.get("rememberPassword").is_none(),
            "the disk shape grew a derived field: {json}"
        );
    }
}
