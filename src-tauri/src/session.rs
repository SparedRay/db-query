//! Connection lifecycle and shared application state.
//!
//! # Connection model (Stage 1)
//!
//! `N + 2` connections for `N` script tabs:
//!
//!   * one **exec** connection per tab, opened lazily on that tab's first run
//!     — idle tabs cost nothing;
//!   * one shared **killer**, which issues `KILL QUERY` and nothing else;
//!   * one shared **meta**, for `information_schema` introspection.
//!
//! Killer and meta are deliberately separate. Cancellation is the feature this
//! whole project exists for, so nothing may be able to block it — and
//! introspection can be slow on servers with thousands of tables.
//!
//! # Lock discipline
//!
//! Stage 0 shipped a deadlock: one mutex covered both the exec and killer
//! connections, so `cancel_query` could never acquire it to kill the query
//! holding it. The same trap scales up here, so the rules are explicit:
//!
//! 1. **Never hold the `tabs` or `conn` map lock across an `await`.** Clone the
//!    `Arc` out, drop the guard, then do the work. Holding it would serialize
//!    every tab through one mutex and undo the entire point of this stage.
//! 2. **`cancel_query` touches only `killer` and the tab's `conn_id`.** It must
//!    never take that tab's `exec` lock — the running query holds it.
//! 3. **The killer connection does nothing else.** If it can be busy, cancel
//!    can be blocked.
//! 4. **Lock order where two are needed: `tabs` → `TabSession.exec` → `conn`.**

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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// In memory only. v1 deliberately does not persist credentials.
    pub password: String,
    pub database: Option<String>,
    /// Accept a self-signed / internal server certificate. Off by default.
    #[serde(default)]
    pub allow_invalid_certs: bool,
}

fn default_port() -> u16 {
    3306
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnInfo {
    pub server_version: String,
    pub databases: Vec<String>,
    pub current_database: Option<String>,
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

/// Server-wide state: the credentials needed to open more connections, the two
/// shared connections, and the schema cache (which is a property of the server,
/// not of any tab).
pub struct ServerConn {
    pub config: ConnConfig,
    pub killer: Mutex<MySqlConnection>,
    pub meta: Mutex<MySqlConnection>,
    pub schema_cache: Mutex<HashMap<String, DbSchema>>,
    pub host_label: String,
    pub server_version: String,
    /// Counts information_schema round trips. The cache's whole job is to keep
    /// this flat on repeat reads, so tests assert on it directly rather than
    /// inferring from timing.
    pub introspection_count: AtomicU64,
}

/// Per-tab state. Everything a script needs to run independently of its
/// siblings: its own connection, its own active database, its own cancel flag.
#[derive(Default)]
pub struct TabSession {
    pub exec: Mutex<Option<MySqlConnection>>,
    pub conn_id: AtomicU64,
    pub current_db: Mutex<Option<String>>,
    pub running: AtomicBool,
    pub cancel_requested: AtomicBool,
}

#[derive(Default)]
pub struct AppState {
    pub conn: Mutex<Option<Arc<ServerConn>>>,
    pub tabs: Mutex<HashMap<TabId, Arc<TabSession>>>,
}

/// sqlx errors are verbose and full of internals. Surface the part a human
/// can act on.
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

fn options(cfg: &ConnConfig) -> MySqlConnectOptions {
    let mut o = MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.user)
        .password(&cfg.password)
        // Secure by default: verify the CA chain and the hostname. The toggle
        // downgrades to "encrypted but unverified", which is what an internal
        // server with a self-signed cert needs — it never falls back to
        // plaintext.
        .ssl_mode(if cfg.allow_invalid_certs {
            MySqlSslMode::Required
        } else {
            MySqlSslMode::VerifyIdentity
        });
    if let Some(db) = cfg.database.as_deref().filter(|d| !d.is_empty()) {
        o = o.database(db);
    }
    o
}

async fn open(cfg: &ConnConfig) -> Result<MySqlConnection, String> {
    MySqlConnection::connect_with(&options(cfg))
        .await
        .map_err(|e| friendly(&e))
}

/// Quote a database identifier for statements that cannot take a bind
/// parameter (`USE`). Internal backticks are doubled; NUL is rejected outright.
pub fn quote_ident(name: &str) -> Result<String, String> {
    if name.contains('\0') {
        return Err("Invalid database name.".into());
    }
    Ok(format!("`{}`", name.replace('`', "``")))
}

// ------------------------------------------------------------------ lookups

/// The active server, or an error if not connected. Clones the `Arc` out and
/// releases the map lock immediately — see rule 1.
pub async fn server(state: &AppState) -> Result<Arc<ServerConn>, String> {
    state
        .conn
        .lock()
        .await
        .as_ref()
        .cloned()
        .ok_or_else(|| "Not connected.".into())
}

/// Look up a tab, creating its (connectionless) session if we have not seen it.
///
/// Auto-creating is deliberate: it removes any ordering race between the
/// frontend registering a tab and that tab's first query.
pub async fn tab(state: &AppState, id: &str) -> Arc<TabSession> {
    let mut guard = state.tabs.lock().await;
    guard.entry(id.to_string()).or_default().clone()
}

/// Look up a tab without creating one.
pub async fn tab_if_open(state: &AppState, id: &str) -> Option<Arc<TabSession>> {
    state.tabs.lock().await.get(id).cloned()
}

// ------------------------------------------------------------- connect/close

pub async fn connect(state: &AppState, cfg: ConnConfig) -> Result<ConnInfo, String> {
    // Replace any existing session cleanly rather than leaking connections.
    disconnect(state).await.ok();

    // Only the two shared connections up front. Tab connections are lazy, so
    // connecting costs 2 regardless of how many tabs are open.
    let mut meta = open(&cfg).await?;
    let killer = open(&cfg).await?;

    let server_version: String = sqlx::query("SELECT VERSION()")
        .fetch_one(&mut meta)
        .await
        .map_err(|e| friendly(&e))?
        .try_get(0)
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

    let current_database = cfg.database.clone().filter(|d| !d.is_empty());
    let host_label = format!("{}@{}:{}", cfg.user, cfg.host, cfg.port);

    *state.conn.lock().await = Some(Arc::new(ServerConn {
        config: cfg,
        killer: Mutex::new(killer),
        meta: Mutex::new(meta),
        schema_cache: Mutex::new(HashMap::new()),
        host_label,
        server_version: server_version.clone(),
        introspection_count: AtomicU64::new(0),
    }));

    Ok(ConnInfo {
        server_version,
        databases,
        current_database,
    })
}

pub async fn disconnect(state: &AppState) -> Result<(), String> {
    // Tabs first: their connections are the ones that leak if we skip them.
    let tabs: Vec<Arc<TabSession>> = state.tabs.lock().await.drain().map(|(_, t)| t).collect();
    for t in tabs {
        if let Some(c) = t.exec.lock().await.take() {
            c.close().await.ok();
        }
        t.conn_id.store(0, Ordering::SeqCst);
    }

    if let Some(server) = state.conn.lock().await.take() {
        // If nobody else is mid-operation, say goodbye properly. Otherwise the
        // last Arc holder dropping it closes the sockets anyway — either way
        // nothing leaks.
        if let Ok(s) = Arc::try_unwrap(server) {
            s.killer.into_inner().close().await.ok();
            s.meta.into_inner().close().await.ok();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- tab lifecycle

pub async fn open_tab(state: &AppState, id: &str) -> Result<(), String> {
    tab(state, id).await;
    Ok(())
}

/// Close a tab and release its connection.
///
/// If a query is in flight we KILL it first: otherwise taking the `exec` lock
/// would block until a 30-second query finished, and closing a tab would hang
/// the UI.
pub async fn close_tab(state: &AppState, id: &str) -> Result<(), String> {
    let Some(tab) = state.tabs.lock().await.remove(id) else {
        return Ok(());
    };
    tab.cancel_requested.store(true, Ordering::SeqCst);
    if tab.running.load(Ordering::SeqCst) {
        kill(state, tab.conn_id.load(Ordering::SeqCst)).await.ok();
    }
    if let Some(c) = tab.exec.lock().await.take() {
        c.close().await.ok();
    }
    Ok(())
}

pub async fn tab_status(state: &AppState, id: &str) -> Result<TabStatus, String> {
    let connected = state.conn.lock().await.is_some();
    let Some(tab) = tab_if_open(state, id).await else {
        return Ok(TabStatus {
            connected,
            running: false,
            current_database: None,
            connection_id: 0,
        });
    };
    // Bind first: a guard temporary inside the struct literal would outlive
    // the borrow of `tab`.
    let current_database = tab.current_db.lock().await.clone();
    Ok(TabStatus {
        connected,
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
    server: &ServerConn,
) -> Result<(), String> {
    if let Some(c) = guard.as_mut() {
        if c.ping().await.is_ok() {
            return Ok(());
        }
        // Reaped or broken. Drop it and open a replacement.
        *guard = None;
        tab.conn_id.store(0, Ordering::SeqCst);
    }

    let want_db = tab.current_db.lock().await.clone();
    let mut cfg = server.config.clone();
    if want_db.is_some() {
        cfg.database = want_db;
    }

    let mut c = open(&cfg).await?;
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

pub async fn use_database(state: &AppState, tab_id: &str, db: &str) -> Result<(), String> {
    let server = server(state).await?;
    let tab = tab(state, tab_id).await;
    let quoted = quote_ident(db)?;

    let mut guard = tab.exec.lock().await;
    ensure_exec(&mut guard, &tab, &server).await?;
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

/// Issue `KILL QUERY <id>` from the shared killer connection.
async fn kill(state: &AppState, conn_id: u64) -> Result<(), String> {
    if conn_id == 0 {
        return Ok(()); // this tab has never opened a connection
    }
    let server = server(state).await?;
    let mut killer = server.killer.lock().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("KILL QUERY {conn_id}")))
        .execute(&mut *killer)
        .await
        .map_err(|e| friendly(&e))?;
    Ok(())
}

/// Cancel the query running in one tab, and only that tab.
///
/// Takes the killer lock and the tab's `conn_id`. It must never take the tab's
/// `exec` lock — the query being killed is holding it.
pub async fn cancel_query(state: &AppState, tab_id: &str) -> Result<(), String> {
    let Some(tab) = tab_if_open(state, tab_id).await else {
        return Ok(());
    };
    // Flag first: even if KILL races the query finishing, the script stops.
    tab.cancel_requested.store(true, Ordering::SeqCst);
    kill(state, tab.conn_id.load(Ordering::SeqCst)).await
}
