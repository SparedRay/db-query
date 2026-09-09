//! MySQL as one engine among several.
//!
//! Deliberately thin: the queries and the caching did not move, they were
//! renamed. `MysqlEngine` is a dispatch vtable that points at the `mysql_*`
//! functions in `schema.rs` and `exec.rs`, which are the same code that has run
//! since Stage 0.
//!
//! That is what let the entire existing suite pass through this refactor
//! untouched — the seam was added, nothing behind it was rewritten.

use crate::engine::{Capabilities, Engine};
use crate::exec::ScriptResult;
use crate::schema::{self, ColumnInfo, RoutineKind, RoutineRef, TableRef};
use crate::session::{AppState, ServerConn};

pub struct MysqlEngine {
    capabilities: Capabilities,
}

impl MysqlEngine {
    pub fn new() -> Self {
        Self {
            capabilities: Capabilities::mysql(),
        }
    }
}

impl Default for MysqlEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Engine for MysqlEngine {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn namespaces(&self, server: &ServerConn) -> Result<Vec<String>, String> {
        crate::session::mysql_databases(server).await
    }

    async fn tables(&self, server: &ServerConn, ns: &str) -> Result<Vec<TableRef>, String> {
        schema::mysql_list_tables(server, ns).await
    }

    async fn columns(
        &self,
        server: &ServerConn,
        ns: &str,
        table: &str,
    ) -> Result<Vec<ColumnInfo>, String> {
        schema::mysql_list_columns(server, ns, table).await
    }

    async fn routines(&self, server: &ServerConn, ns: &str) -> Result<Vec<RoutineRef>, String> {
        schema::mysql_list_routines(server, ns).await
    }

    async fn table_ddl(
        &self,
        server: &ServerConn,
        ns: &str,
        table: &str,
    ) -> Result<String, String> {
        schema::mysql_table_ddl(server, ns, table).await
    }

    async fn routine_ddl(
        &self,
        server: &ServerConn,
        ns: &str,
        name: &str,
        kind: RoutineKind,
    ) -> Result<String, String> {
        schema::mysql_routine_ddl(server, ns, name, kind).await
    }

    async fn run_script(
        &self,
        state: &AppState,
        tab_id: &str,
        sql: &str,
        auto_limit: bool,
        timeout_secs: Option<u64>,
    ) -> Result<ScriptResult, String> {
        crate::exec::mysql_run_script(state, tab_id, sql, auto_limit, timeout_secs).await
    }

    async fn use_namespace(&self, state: &AppState, tab_id: &str, ns: &str) -> Result<(), String> {
        crate::session::mysql_use_database(state, tab_id, ns).await
    }

    async fn cancel(&self, state: &AppState, tab_id: &str) -> Result<(), String> {
        crate::session::mysql_cancel_query(state, tab_id).await
    }
}
