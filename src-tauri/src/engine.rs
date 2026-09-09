//! What an engine can do, as data.
//!
//! # Why this exists before a second engine does
//!
//! The thing that makes a third engine expensive is not the dispatch — it is
//! `if backend.is_elastic()` scattered through `exec.rs`, the schema tree, the
//! assistant and export. Every one of those is a place a future engine has to
//! be remembered in, and forgetting one is a bug nobody sees until someone
//! connects.
//!
//! Expressed as capabilities instead, an engine *declares* what it supports and
//! the call sites ask. Adding an engine becomes an addition rather than a hunt.
//! See §3 of the Stage 11 tracker for the reasoning, including the enum-shaped
//! recommendation this replaced.
//!
//! These are deliberately **capabilities, not an engine name**. `caps.writes`
//! survives a fourth engine that happens to allow writes; `engine == "mysql"`
//! does not.

use serde::Serialize;

/// How a result set is bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowCap {
    /// The client appends `LIMIT`, and says so — MySQL's auto-LIMIT.
    ClientLimit,
    /// The server takes a page size and hands back a cursor; the client never
    /// rewrites the statement. Elasticsearch's `fetch_size`.
    ServerPageSize,
}

/// What one engine supports. Travels with `ConnInfo`, so the **frontend reads
/// it too** — a menu that offers `DROP` against a read-only engine is a bug the
/// backend cannot prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Which engine this is. For display and for the assistant's dialect —
    /// **never** for behaviour. Anything that branches on this belongs in a
    /// capability instead.
    pub engine: String,
    /// Statements that change data or schema are accepted at all.
    pub writes: bool,
    /// `BEGIN` / `COMMIT` / `ROLLBACK` mean something.
    pub transactions: bool,
    /// More than one statement can be sent in a script.
    pub multi_statement: bool,
    /// `DELIMITER $$` blocks are understood.
    pub delimiter_blocks: bool,
    /// Stored procedures and functions exist and can be listed.
    pub routines: bool,
    /// A running statement can be stopped.
    pub cancellation: bool,
    /// Export can stream row-at-a-time rather than holding the whole result.
    pub streaming_export: bool,
    pub row_cap: RowCap,
    /// What this engine calls the thing `USE` switches between. MySQL says
    /// "database", Elasticsearch says "catalog", Snowflake says "schema" —
    /// borrowing MySQL's word for all of them puts a small lie in the UI.
    pub namespace_label: String,
}

impl Capabilities {
    /// Everything this app has ever been able to do, because until now it has
    /// only spoken to MySQL. Written out in full rather than derived from a
    /// `Default`, so that adding a capability forces a decision here.
    pub fn mysql() -> Self {
        Self {
            engine: "mysql".into(),
            writes: true,
            transactions: true,
            multi_statement: true,
            delimiter_blocks: true,
            routines: true,
            cancellation: true,
            streaming_export: true,
            row_cap: RowCap::ClientLimit,
            namespace_label: "database".into(),
        }
    }
}

/// One engine's implementation of everything that differs between engines.
///
/// **A trait rather than an enum**, because a third engine (Snowflake,
/// PostgreSQL) is plausible and should be an *addition* — one file implementing
/// one interface — rather than a hunt for every match arm. See §3 of the Stage
/// 11 tracker for the reasoning, including the enum this replaced.
///
/// Methods take `&ServerConn` rather than owning their transport, because MySQL
/// genuinely needs shared connections that the session layer already stores and
/// closes; Elasticsearch keeps everything it needs in `self` and ignores the
/// argument. That keeps this a dispatch layer and leaves the connection
/// lifecycle in one place.
///
/// **Nothing here branches on `capabilities().engine`.** That field names the
/// engine for display; behaviour is dispatched by *being* an implementation.
#[async_trait::async_trait]
pub trait Engine: Send + Sync {
    fn capabilities(&self) -> &Capabilities;

    /// Databases, catalogs or schemas — whatever `namespace_label` calls them.
    async fn namespaces(&self, server: &crate::session::ServerConn) -> Result<Vec<String>, String>;

    async fn tables(
        &self,
        server: &crate::session::ServerConn,
        ns: &str,
    ) -> Result<Vec<crate::schema::TableRef>, String>;

    async fn columns(
        &self,
        server: &crate::session::ServerConn,
        ns: &str,
        table: &str,
    ) -> Result<Vec<crate::schema::ColumnInfo>, String>;

    /// Engines without stored routines return an empty list rather than an
    /// error — the tree omits empty groups, so nothing has to know why.
    async fn routines(
        &self,
        server: &crate::session::ServerConn,
        ns: &str,
    ) -> Result<Vec<crate::schema::RoutineRef>, String>;

    async fn table_ddl(
        &self,
        server: &crate::session::ServerConn,
        ns: &str,
        table: &str,
    ) -> Result<String, String>;

    async fn routine_ddl(
        &self,
        server: &crate::session::ServerConn,
        ns: &str,
        name: &str,
        kind: crate::schema::RoutineKind,
    ) -> Result<String, String>;

    /// Run a whole script.
    ///
    /// The engine owns the loop because the session state differs so much: a
    /// MySQL tab has its own connection, temp tables and open transaction, while
    /// an Elasticsearch tab has nothing at all. Both share the splitting,
    /// classification and refusal in `exec`.
    async fn run_script(
        &self,
        state: &crate::session::AppState,
        tab_id: &str,
        sql: &str,
        auto_limit: bool,
        timeout_secs: Option<u64>,
    ) -> Result<crate::exec::ScriptResult, String>;

    /// Make this the tab's current namespace. A no-op where switching is free.
    async fn use_namespace(
        &self,
        state: &crate::session::AppState,
        tab_id: &str,
        ns: &str,
    ) -> Result<(), String>;

    /// Stop whatever the tab is running. Best-effort by nature.
    async fn cancel(&self, state: &crate::session::AppState, tab_id: &str) -> Result<(), String>;

    /// This dialect's spelling of an identifier.
    ///
    /// MySQL uses backticks, Elasticsearch and the SQL standard use double
    /// quotes, and Snowflake would use double quotes too — so this belongs to
    /// the engine, not to a shared generator that happened to be written first.
    fn quote_ident(&self, name: &str) -> Result<String, String>;

    /// The character [`Engine::quote_ident`] wraps a name in.
    ///
    /// Defaulted to the standard `"`, which is what Elasticsearch uses and what
    /// MySQL reads as a *string* — so an engine that disagrees must say so. The
    /// linter needs the character rather than the quoting function: it works on
    /// a masked copy of the buffer and has to know which quotes hold names.
    fn ident_quote(&self) -> char {
        '"'
    }

    /// How a table is named when a namespace is in play.
    ///
    /// Defaulted to `ns.table` because that is what most engines do. An engine
    /// whose namespaces are not a table prefix — Elasticsearch's catalogs are
    /// remote clusters, not schemas — overrides it and drops the prefix.
    fn qualify(&self, ns: &str, table: &str) -> Result<String, String> {
        Ok(format!(
            "{}.{}",
            self.quote_ident(ns)?,
            self.quote_ident(table)?
        ))
    }

    /// The double-click action: browse a table's first rows.
    ///
    /// A default rather than free-standing code, so a new engine gets something
    /// correct without writing anything, and can still override when its
    /// dialect disagrees — `LIMIT` is not universal (Snowflake and SQL Server
    /// spell it differently), and this is the hook where that is said.
    fn select_snippet(&self, ns: &str, table: &str, limit: u32) -> Result<String, String> {
        if limit == 0 {
            return Err("A browse limit of 0 would return nothing.".into());
        }
        Ok(format!(
            "SELECT *\nFROM {}\nLIMIT {limit};\n",
            self.qualify(ns, table)?
        ))
    }
}

/// Why this engine will not run a statement, if it will not.
///
/// Checked **before** the statement is sent. Relaying a remote parser's
/// complaint about `UPDATE` is worse than saying plainly that the engine has no
/// writes — and for the engines that do, this returns `None` and nothing
/// changes.
pub fn refusal(caps: &Capabilities, kind: crate::exec::StatementKind) -> Option<String> {
    use crate::exec::StatementKind;
    match kind {
        StatementKind::Modify | StatementKind::Other if !caps.writes => Some(format!(
            "{} is read-only through this interface — it accepts SELECT, SHOW and DESCRIBE.",
            caps.engine
        )),
        StatementKind::Session if !caps.transactions => Some(format!(
            "{} has no transactions or session statements.",
            caps.engine
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MySQL must keep declaring everything the app already does, or Stage 11
    /// silently takes features away from the engine it was meant to leave alone.
    #[test]
    fn mysql_declares_every_capability() {
        let c = Capabilities::mysql();
        assert!(c.writes);
        assert!(c.transactions);
        assert!(c.multi_statement);
        assert!(c.delimiter_blocks);
        assert!(c.routines);
        assert!(c.cancellation);
        assert!(c.streaming_export);
        assert_eq!(c.row_cap, RowCap::ClientLimit);
        assert_eq!(c.namespace_label, "database");
    }

    /// The frontend matches on these names; renaming one silently would leave
    /// menus permanently hidden or permanently offered.
    #[test]
    fn capabilities_serialise_with_the_names_the_ui_expects() {
        let j = serde_json::to_value(Capabilities::mysql()).unwrap();
        for key in [
            "engine",
            "writes",
            "transactions",
            "multiStatement",
            "delimiterBlocks",
            "routines",
            "cancellation",
            "streamingExport",
            "rowCap",
            "namespaceLabel",
        ] {
            assert!(j.get(key).is_some(), "missing {key}: {j}");
        }
        assert_eq!(j["rowCap"], "clientLimit");
    }

    // ---------------------------------------------------------- refusals

    fn read_only() -> Capabilities {
        Capabilities {
            engine: "elasticsearch".into(),
            writes: false,
            transactions: false,
            multi_statement: true,
            delimiter_blocks: false,
            routines: false,
            cancellation: true,
            streaming_export: false,
            row_cap: RowCap::ServerPageSize,
            namespace_label: "catalog".into(),
        }
    }

    /// The whole point of doing this from capabilities: MySQL is unaffected,
    /// and stays unaffected when a fourth engine appears.
    #[test]
    fn an_engine_with_writes_refuses_nothing() {
        let caps = Capabilities::mysql();
        for kind in [
            crate::exec::StatementKind::Select,
            crate::exec::StatementKind::RowReturning,
            crate::exec::StatementKind::Modify,
            crate::exec::StatementKind::Session,
            crate::exec::StatementKind::Other,
        ] {
            assert_eq!(refusal(&caps, kind), None, "{kind:?}");
        }
    }

    #[test]
    fn a_read_only_engine_refuses_writes_and_says_what_it_does_take() {
        let caps = read_only();
        let message =
            refusal(&caps, crate::exec::StatementKind::Modify).expect("a write must be refused");
        assert!(message.contains("read-only"), "{message}");
        assert!(message.contains("SELECT"), "{message}");
        assert!(message.contains("elasticsearch"), "{message}");
    }

    #[test]
    fn a_read_only_engine_still_runs_reads() {
        let caps = read_only();
        assert_eq!(refusal(&caps, crate::exec::StatementKind::Select), None);
        assert_eq!(
            refusal(&caps, crate::exec::StatementKind::RowReturning),
            None
        );
    }

    /// Transactions are their own capability: an engine could accept writes and
    /// still have no `BEGIN`.
    #[test]
    fn transactions_are_refused_separately_from_writes() {
        let mut caps = Capabilities::mysql();
        caps.transactions = false;
        assert!(refusal(&caps, crate::exec::StatementKind::Session).is_some());
        assert_eq!(refusal(&caps, crate::exec::StatementKind::Modify), None);
    }
}
