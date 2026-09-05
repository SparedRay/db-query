//! Connection lifecycle and shared application state.
//!
//! Two dedicated `MySqlConnection`s, deliberately not a `MySqlPool`:
//!   * `CONNECTION_ID()` must belong to the connection actually running the
//!     query, or `KILL QUERY` targets a stranger's session.
//!   * `USE db`, temp tables and session variables have to persist across
//!     statements in a script, which a pool does not guarantee.
//!
//! The two connections live behind *separate* mutexes. `run_script` holds the
//! `exec` lock for as long as the query runs; if the killer connection shared
//! that lock, `cancel_query` could never acquire it and cancellation would
//! deadlock against the very query it exists to kill.
//!
//! Lock order is always `exec` then `ctl`. Never the reverse.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
use sqlx::{Connection, MySqlConnection, Row};
use tokio::sync::Mutex;

use crate::schema::DbSchema;

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
    pub connection_id: u64,
}

/// Everything that must stay reachable *while* a query is in flight.
pub struct Control {
    pub killer: MySqlConnection,
    pub exec_conn_id: u64,
    pub current_db: Option<String>,
    pub schema_cache: HashMap<String, DbSchema>,
    pub host_label: String,
}

#[derive(Default)]
pub struct AppState {
    pub exec: Mutex<Option<MySqlConnection>>,
    pub ctl: Mutex<Option<Control>>,
    pub running: AtomicBool,
    /// Set by `cancel_query`, read by `run_script` between statements so the
    /// remainder of a script is abandoned after a KILL — matching the
    /// stop-at-first-error rule.
    pub cancel_requested: AtomicBool,
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

pub async fn connect(state: &AppState, cfg: ConnConfig) -> Result<ConnInfo, String> {
    // Replace any existing session cleanly rather than leaking connections.
    disconnect(state).await.ok();

    let mut exec = open(&cfg).await?;
    let killer = open(&cfg).await?;

    // Prove the link works, and capture the id KILL QUERY will target.
    let conn_id: u64 = sqlx::query("SELECT CONNECTION_ID()")
        .fetch_one(&mut exec)
        .await
        .map_err(|e| friendly(&e))?
        .try_get(0)
        .map_err(|e| friendly(&e))?;

    let server_version: String = sqlx::query("SELECT VERSION()")
        .fetch_one(&mut exec)
        .await
        .map_err(|e| friendly(&e))?
        .try_get(0)
        .map_err(|e| friendly(&e))?;

    let databases: Vec<String> =
        sqlx::query("SELECT schema_name FROM information_schema.schemata ORDER BY schema_name")
            .fetch_all(&mut exec)
            .await
            .map_err(|e| friendly(&e))?
            .into_iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect();

    let current_database = cfg.database.clone().filter(|d| !d.is_empty());
    let host_label = format!("{}@{}:{}", cfg.user, cfg.host, cfg.port);

    *state.exec.lock().await = Some(exec);
    *state.ctl.lock().await = Some(Control {
        killer,
        exec_conn_id: conn_id,
        current_db: current_database.clone(),
        schema_cache: HashMap::new(),
        host_label,
    });

    Ok(ConnInfo {
        server_version,
        databases,
        current_database,
        connection_id: conn_id,
    })
}

pub async fn disconnect(state: &AppState) -> Result<(), String> {
    if let Some(c) = state.exec.lock().await.take() {
        c.close().await.ok();
    }
    if let Some(ctl) = state.ctl.lock().await.take() {
        ctl.killer.close().await.ok();
    }
    Ok(())
}

/// Test-only: drop the exec connection while leaving `ctl` — and therefore the
/// schema cache — intact. Lets a test prove a cached read needs no server
/// round trip, rather than just asserting it returns the same data twice.
#[doc(hidden)]
pub async fn drop_exec_for_test(state: &AppState) {
    if let Some(c) = state.exec.lock().await.take() {
        c.close().await.ok();
    }
}

pub async fn use_database(state: &AppState, db: &str) -> Result<(), String> {
    let mut guard = state.exec.lock().await;
    let conn = guard.as_mut().ok_or("Not connected.")?;

    // `USE` cannot take a bind parameter, so the identifier is quoted and
    // internal backticks doubled. Rejecting NUL keeps the escape airtight.
    if db.contains('\0') {
        return Err("Invalid database name.".into());
    }
    let quoted = format!("`{}`", db.replace('`', "``"));
    // Identifier is backtick-quoted with internal backticks doubled, and NUL
    // is rejected above, so this interpolation is audited safe.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("USE {quoted}")))
        .execute(&mut *conn)
        .await
        .map_err(|e| friendly(&e))?;
    drop(guard);

    if let Some(ctl) = state.ctl.lock().await.as_mut() {
        ctl.current_db = Some(db.to_string());
    }
    Ok(())
}

/// Issue `KILL QUERY <exec connection id>` from the idle killer connection.
/// Takes only the `ctl` lock, so it works while `run_script` holds `exec`.
pub async fn cancel_query(state: &AppState) -> Result<(), String> {
    // Flag first: even if KILL races the query finishing, the script stops.
    state
        .cancel_requested
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut guard = state.ctl.lock().await;
    let ctl = guard.as_mut().ok_or("Not connected.")?;
    let id = ctl.exec_conn_id;
    // `id` is a u64 read back from the server; no injection surface.
    sqlx::query(sqlx::AssertSqlSafe(format!("KILL QUERY {id}")))
        .execute(&mut ctl.killer)
        .await
        .map_err(|e| friendly(&e))?;
    Ok(())
}
