//! Elasticsearch SQL, over `POST /_sql`.
//!
//! # What this engine is not
//!
//! Verified against the API reference on 2026-09-08: Elasticsearch SQL accepts
//! **`SELECT`, `SHOW` and `DESCRIBE` and nothing else**. No DDL, no
//! INSERT/UPDATE/DELETE, no transactions. That is declared in
//! [`capabilities`] rather than enforced by name checks, so the refusal lives
//! once in `engine::refusal` and the tree menus read the same flags.
//!
//! # Rows, and why the statement is never rewritten
//!
//! MySQL gets an auto-`LIMIT` appended, which Stage 9 established must never be
//! recorded as the user's SQL. Elasticsearch has a first-class page size, so it
//! is used instead: `fetch_size` caps the first page, and a `cursor` coming back
//! means there was more — which sets the same `truncated` flag the grid has
//! always rendered. The cursor is then **closed**, because a page left open is
//! server-side state we asked for and abandoned.

use serde::Serialize;

use crate::decode::CellValue;
use crate::engine::{Capabilities, RowCap};
use crate::exec::Outcome;
use crate::exec::{classify, ScriptResult, StatementResult};
use crate::httpsql::{self, Auth};
use crate::schema::{ColumnInfo, RoutineKind, RoutineRef, TableRef};
use crate::session::{AppState, ServerConn};

/// What Elasticsearch SQL supports, and what it does not.
pub fn capabilities() -> Capabilities {
    Capabilities {
        engine: "elasticsearch".into(),
        // Verified from the API reference: SELECT only.
        writes: false,
        transactions: false,
        // One query per request, but a script of several is several requests.
        multi_statement: true,
        delimiter_blocks: false,
        routines: false,
        cancellation: true,
        // Results arrive whole; there is no row-at-a-time source to stream from.
        streaming_export: false,
        row_cap: RowCap::ServerPageSize,
        // Elasticsearch's own word, and the name of the request field.
        namespace_label: "catalog".into(),
    }
}

/// A cluster we can talk to.
pub struct ElasticEngine {
    client: reqwest::Client,
    /// Normalised without a trailing slash, so paths concatenate cleanly.
    base: String,
    auth: Auth,
    secret: Option<String>,
    capabilities: Capabilities,
}

/// The `_sql` response, in the shape the reference documents.
#[derive(Debug, Default)]
pub struct SqlPage {
    pub columns: Vec<serde_json::Value>,
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Present only when there is another page — which is how "there was more"
    /// is detected without asking for a count.
    pub cursor: Option<String>,
}

#[derive(Serialize)]
struct SqlRequest<'a> {
    query: &'a str,
    fetch_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<&'a str>,
}

pub fn normalise_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// Parse a `_sql` response body.
///
/// Separated from the request so it can be tested against recorded payloads,
/// which is the only way to cover the shapes a live cluster rarely produces.
pub fn parse_page(v: &serde_json::Value) -> SqlPage {
    SqlPage {
        columns: v
            .get("columns")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default(),
        rows: v
            .get("rows")
            .and_then(|r| r.as_array())
            .map(|rows| {
                rows.iter()
                    .map(|r| r.as_array().cloned().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default(),
        cursor: v
            .get("cursor")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(str::to_owned),
    }
}

/// Turn one page into the outcome the grid already renders.
///
/// **A follow-on cursor means truncation, not an error.** The app's row budget
/// is the reason there is a second page at all, so saying so with the existing
/// flag is more honest than pretending the result was complete.
pub fn page_to_outcome(page: &SqlPage) -> Outcome {
    Outcome::Rows {
        columns: page.columns.iter().map(httpsql::column_from_json).collect(),
        rows: page
            .rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(httpsql::cell_from_json)
                    .collect::<Vec<CellValue>>()
            })
            .collect(),
        truncated: page.cursor.is_some(),
    }
}

/// `SHOW TABLES` gives a name and a type per index. Column names differ between
/// versions ("name"/"table", "type"/"kind"), so both are accepted rather than
/// pinning one and breaking on the other.
pub fn tables_from_page(page: &SqlPage) -> Vec<TableRef> {
    let idx = |wanted: &[&str]| -> Option<usize> {
        page.columns.iter().position(|c| {
            c.get("name")
                .and_then(|n| n.as_str())
                .map(|n| wanted.contains(&n.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
    };
    let name_at = idx(&["name", "table"]).unwrap_or(0);
    let kind_at = idx(&["type", "kind"]);

    page.rows
        .iter()
        .filter_map(|row| {
            let name = row.get(name_at)?.as_str()?.to_string();
            let kind = kind_at
                .and_then(|i| row.get(i))
                .and_then(|k| k.as_str())
                .unwrap_or("TABLE")
                .to_string();
            // An alias behaves like a view: readable, not a base table.
            let kind = if kind.to_ascii_uppercase().contains("ALIAS") {
                "VIEW".to_string()
            } else {
                kind
            };
            Some(TableRef { name, kind })
        })
        .collect()
}

/// `DESCRIBE <index>` gives column, type and mapping.
///
/// Everything in Elasticsearch is nullable and nothing is a primary key, so
/// those fields are reported honestly rather than invented.
pub fn columns_from_page(page: &SqlPage) -> Vec<ColumnInfo> {
    page.rows
        .iter()
        .filter_map(|row| {
            let name = row.first()?.as_str()?.to_string();
            let data_type = row
                .get(1)
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some(ColumnInfo {
                name,
                data_type,
                nullable: true,
                key: None,
            })
        })
        .collect()
}

impl ElasticEngine {
    pub fn new(base: &str, auth: Auth, secret: Option<String>) -> Self {
        // Here rather than in the caller: `reqwest` is built with
        // `rustls-no-provider`, so building a client without a provider panics.
        // `run()` installs one at startup, but an engine constructed by a test —
        // or by anything else that is not the app — would still panic, and did.
        crate::install_tls();
        Self {
            client: reqwest::Client::new(),
            base: normalise_base(base),
            auth,
            secret,
            capabilities: capabilities(),
        }
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.auth.apply(
            self.client
                .post(format!("{}{path}", self.base))
                .header("content-type", "application/json"),
            self.secret.as_deref(),
        )
    }

    /// Run one statement and take the first page.
    pub async fn query(
        &self,
        sql: &str,
        fetch_size: usize,
        catalog: Option<&str>,
    ) -> Result<SqlPage, String> {
        let body = SqlRequest {
            query: sql,
            fetch_size,
            catalog,
        };
        let response = self
            .post("/_sql?format=json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Could not reach {}: {e}", self.base))?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(httpsql::error_message(status, &text));
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("The cluster returned something unreadable: {e}"))?;
        let page = parse_page(&v);

        // Release the page we are not going to read. Best-effort: failing to
        // close a cursor must not fail the query the user actually asked for.
        if let Some(cursor) = &page.cursor {
            let _ = self
                .post("/_sql/close")
                .json(&serde_json::json!({ "cursor": cursor }))
                .send()
                .await;
        }
        Ok(page)
    }

    /// The cluster's version, which doubles as a reachability check.
    pub async fn version(&self) -> Result<String, String> {
        let response = self
            .auth
            .apply(self.client.get(&self.base), self.secret.as_deref())
            .send()
            .await
            .map_err(|e| format!("Could not reach {}: {e}", self.base))?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(httpsql::error_message(status, &text));
        }
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        Ok(v.get("version")
            .and_then(|v| v.get("number"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string())
    }
}

/// Quote an identifier for Elasticsearch SQL, which uses double quotes.
///
/// An index name can contain almost anything, so this is not optional — and
/// embedded quotes are doubled rather than stripped, because silently changing
/// a name is how you describe the wrong index.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[async_trait::async_trait]
impl crate::engine::Engine for ElasticEngine {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// `SHOW CATALOGS` is the closest thing this engine has to a database list,
    /// and it is the same word its request body uses. A cluster that will not
    /// answer it is not broken — cross-cluster search may simply be off — so the
    /// failure is an empty list rather than a refused connection.
    async fn namespaces(&self, _server: &ServerConn) -> Result<Vec<String>, String> {
        let page = match self.query("SHOW CATALOGS", 100, None).await {
            Ok(p) => p,
            Err(_) => return Ok(Vec::new()),
        };
        Ok(page
            .rows
            .iter()
            .filter_map(|r| r.first()?.as_str().map(str::to_owned))
            .collect())
    }

    async fn tables(&self, server: &ServerConn, ns: &str) -> Result<Vec<TableRef>, String> {
        if let Some(cached) = server
            .schema_cache
            .lock()
            .await
            .get(ns)
            .and_then(|s| s.tables.clone())
        {
            return Ok(cached);
        }
        let catalog = (!ns.is_empty()).then_some(ns);
        let page = self.query("SHOW TABLES", 1000, catalog).await?;
        let tables = tables_from_page(&page);
        server
            .schema_cache
            .lock()
            .await
            .entry(ns.to_string())
            .or_default()
            .tables = Some(tables.clone());
        Ok(tables)
    }

    async fn columns(
        &self,
        server: &ServerConn,
        ns: &str,
        table: &str,
    ) -> Result<Vec<ColumnInfo>, String> {
        if let Some(cached) = server
            .schema_cache
            .lock()
            .await
            .get(ns)
            .and_then(|s| s.columns.get(table))
            .cloned()
        {
            return Ok(cached);
        }
        let catalog = (!ns.is_empty()).then_some(ns);
        let page = self
            .query(&format!("DESCRIBE {}", quote_ident(table)), 1000, catalog)
            .await?;
        let columns = columns_from_page(&page);
        server
            .schema_cache
            .lock()
            .await
            .entry(ns.to_string())
            .or_default()
            .columns
            .insert(table.to_string(), columns.clone());
        Ok(columns)
    }

    /// No stored routines. An empty list rather than an error: the tree omits
    /// empty groups, so nothing above has to know why there are none.
    async fn routines(&self, _server: &ServerConn, _ns: &str) -> Result<Vec<RoutineRef>, String> {
        Ok(Vec::new())
    }

    /// There is no `SHOW CREATE` here. `DESCRIBE` is the nearest true answer,
    /// rendered as a comment block — inventing a `CREATE TABLE` that this engine
    /// could never run would be worse than saying what is actually known.
    async fn table_ddl(
        &self,
        _server: &ServerConn,
        ns: &str,
        table: &str,
    ) -> Result<String, String> {
        let catalog = (!ns.is_empty()).then_some(ns);
        let page = self
            .query(&format!("DESCRIBE {}", quote_ident(table)), 1000, catalog)
            .await?;
        let mut out = format!(
            "-- Elasticsearch has no CREATE statement for an index.\n             -- This is DESCRIBE {}, which is what the cluster will tell us.\n\n",
            quote_ident(table)
        );
        for c in columns_from_page(&page) {
            out.push_str(&format!("--   {:<32} {}\n", c.name, c.data_type));
        }
        out.push_str(&format!("\nDESCRIBE {};\n", quote_ident(table)));
        Ok(out)
    }

    async fn routine_ddl(
        &self,
        _server: &ServerConn,
        _ns: &str,
        _name: &str,
        _kind: RoutineKind,
    ) -> Result<String, String> {
        Err("Elasticsearch has no stored routines.".into())
    }

    /// Run a script, one request per statement.
    ///
    /// Shares the splitting, classification and capability refusal with MySQL —
    /// what differs is only how a single statement is sent, which is the whole
    /// point of the seam. `fetch_size` replaces auto-LIMIT, so **the user's SQL
    /// is never rewritten**: nothing here has an `effective_sql`.
    async fn run_script(
        &self,
        state: &AppState,
        tab_id: &str,
        sql: &str,
        _auto_limit: bool,
        _timeout_secs: Option<u64>,
    ) -> Result<ScriptResult, String> {
        let tab = crate::session::tab(state, tab_id).await?;
        let split_out = crate::split::split(sql);
        let catalog = tab.current_db.lock().await.clone();

        tab.cancel_requested
            .store(false, std::sync::atomic::Ordering::SeqCst);
        tab.running.store(true, std::sync::atomic::Ordering::SeqCst);

        let script_start = std::time::Instant::now();
        let mut statements = Vec::new();
        let mut aborted_at = None;
        let mut cancelled = false;
        let mut budget = crate::exec::MAX_SCRIPT_ROWS;

        for (idx, span) in split_out.statements.iter().enumerate() {
            if tab
                .cancel_requested
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                cancelled = true;
                break;
            }

            let text = &sql[span.start..span.end];
            let kind = classify(text);
            let started = std::time::Instant::now();

            let outcome = match crate::engine::refusal(&self.capabilities, kind) {
                Some(message) => crate::exec::Outcome::Error { message },
                None => {
                    let fetch = budget.min(crate::exec::MAX_ROWS);
                    match self.query(text, fetch.max(1), catalog.as_deref()).await {
                        Ok(page) => {
                            budget = budget.saturating_sub(page.rows.len());
                            page_to_outcome(&page)
                        }
                        Err(message) => crate::exec::Outcome::Error { message },
                    }
                }
            };

            let is_error = matches!(outcome, crate::exec::Outcome::Error { .. });
            statements.push(StatementResult {
                sql: text.to_string(),
                // Never rewritten: the row cap is a request field, not a LIMIT
                // appended to what the user typed.
                effective_sql: None,
                kind,
                outcome,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });

            if is_error {
                aborted_at = Some(idx);
                break;
            }
        }

        let cancelled = cancelled
            || tab
                .cancel_requested
                .load(std::sync::atomic::Ordering::SeqCst);
        tab.running
            .store(false, std::sync::atomic::Ordering::SeqCst);

        Ok(ScriptResult {
            statements,
            total_elapsed_ms: script_start.elapsed().as_millis() as u64,
            aborted_at,
            delimiter_detected: split_out.delimiter_detected,
            cancelled,
            timed_out: false,
        })
    }

    /// Switching catalog is free — it is a field on the next request, not a
    /// round trip — so this only records the choice.
    async fn use_namespace(&self, state: &AppState, tab_id: &str, ns: &str) -> Result<(), String> {
        let tab = crate::session::tab(state, tab_id).await?;
        *tab.current_db.lock().await = Some(ns.to_string());
        Ok(())
    }

    /// There is no server-side kill. Setting the flag stops the script between
    /// statements, which is the honest extent of it — and is why a long single
    /// query cannot be interrupted here.
    async fn cancel(&self, state: &AppState, tab_id: &str) -> Result<(), String> {
        let tab = crate::session::tab(state, tab_id).await?;
        tab.cancel_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::TypeHint;

    fn page(json: serde_json::Value) -> SqlPage {
        parse_page(&json)
    }

    /// The response shape from the API reference, end to end into the grid.
    #[test]
    fn a_result_page_becomes_the_grid_types_the_app_already_has() {
        let p = page(serde_json::json!({
            "columns": [
                {"name": "author", "type": "text"},
                {"name": "page_count", "type": "long"}
            ],
            "rows": [["Dan Simmons", 482], ["Frank Herbert", 604]]
        }));

        match page_to_outcome(&p) {
            Outcome::Rows {
                columns,
                rows,
                truncated,
            } => {
                assert_eq!(columns[0].name, "author");
                assert_eq!(columns[1].sql_type, "long");
                assert_eq!(columns[1].type_hint, TypeHint::Numeric);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][1], CellValue::Int(482));
                assert!(!truncated, "one page is not truncated");
            }
            other => panic!("expected rows, got {other:?}"),
        }
    }

    /// "You've reached the last page when there is no `cursor`" — so a cursor
    /// means there was more, which is exactly what `truncated` says.
    #[test]
    fn a_cursor_means_the_result_was_truncated() {
        let p = page(serde_json::json!({
            "columns": [{"name": "a", "type": "long"}],
            "rows": [[1]],
            "cursor": "sDXF1ZXJ5QW5kRmV0Y2gB"
        }));
        assert_eq!(p.cursor.as_deref(), Some("sDXF1ZXJ5QW5kRmV0Y2gB"));
        match page_to_outcome(&p) {
            Outcome::Rows { truncated, .. } => assert!(truncated),
            other => panic!("expected rows, got {other:?}"),
        }
    }

    /// An empty cursor is not a cursor. Reading one would send a close request
    /// for nothing and claim a truncation that did not happen.
    #[test]
    fn an_empty_cursor_is_no_cursor() {
        let p = page(serde_json::json!({"columns": [], "rows": [], "cursor": ""}));
        assert!(p.cursor.is_none());
    }

    #[test]
    fn a_result_with_no_rows_is_still_a_result() {
        let p = page(serde_json::json!({
            "columns": [{"name": "a", "type": "long"}],
            "rows": []
        }));
        match page_to_outcome(&p) {
            Outcome::Rows { columns, rows, .. } => {
                assert_eq!(columns.len(), 1);
                assert!(rows.is_empty());
            }
            other => panic!("expected rows, got {other:?}"),
        }
    }

    // ------------------------------------------------------------ schema

    #[test]
    fn show_tables_becomes_the_tree_nodes_the_app_already_draws() {
        let p = page(serde_json::json!({
            "columns": [{"name": "name", "type": "keyword"}, {"name": "type", "type": "keyword"}],
            "rows": [["orders", "TABLE"], ["orders_alias", "ALIAS"]]
        }));
        let tables = tables_from_page(&p);
        assert_eq!(tables[0].name, "orders");
        assert_eq!(tables[0].kind, "TABLE");
        // An alias reads like a view: readable, not a base table. The tree
        // already groups on that word.
        assert_eq!(tables[1].kind, "VIEW");
    }

    /// Column names differ between versions. Pinning one and breaking on the
    /// other is the kind of failure that only shows up on someone else's cluster.
    #[test]
    fn show_tables_accepts_either_columns_naming() {
        let p = page(serde_json::json!({
            "columns": [{"name": "table", "type": "keyword"}, {"name": "kind", "type": "keyword"}],
            "rows": [["events", "BASE TABLE"]]
        }));
        let tables = tables_from_page(&p);
        assert_eq!(tables[0].name, "events");
        assert_eq!(tables[0].kind, "BASE TABLE");
    }

    #[test]
    fn describe_becomes_columns_without_inventing_keys() {
        let p = page(serde_json::json!({
            "columns": [
                {"name": "column", "type": "keyword"},
                {"name": "type", "type": "keyword"},
                {"name": "mapping", "type": "keyword"}
            ],
            "rows": [["total", "DOUBLE", "double"], ["user", "VARCHAR", "keyword"]]
        }));
        let cols = columns_from_page(&p);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "total");
        assert_eq!(cols[0].data_type, "DOUBLE");
        // Everything is nullable and nothing is a key. Saying otherwise would
        // put MySQL's shape onto an engine that does not have it.
        assert!(cols[0].nullable);
        assert!(cols[0].key.is_none());
    }

    // ------------------------------------------------------- capabilities

    /// The declared capabilities are what the refusal and the menus read, so
    /// they are the whole contract for what this engine will not do.
    #[test]
    fn the_engine_declares_itself_read_only() {
        let c = capabilities();
        assert!(!c.writes);
        assert!(!c.transactions);
        assert!(!c.routines);
        assert!(!c.delimiter_blocks);
        assert!(!c.streaming_export);
        assert_eq!(c.row_cap, RowCap::ServerPageSize);
        assert_eq!(c.namespace_label, "catalog");
    }

    /// A script of several SELECTs is several requests, which is supported —
    /// it is *one query per request* that is the constraint, not one per script.
    #[test]
    fn several_statements_are_allowed() {
        assert!(capabilities().multi_statement);
    }

    #[test]
    fn a_base_url_normalises_regardless_of_a_trailing_slash() {
        assert_eq!(
            normalise_base("http://localhost:9200/"),
            "http://localhost:9200"
        );
        assert_eq!(normalise_base("  http://es:9200  "), "http://es:9200");
    }

    /// An index name can contain almost anything; an embedded quote is doubled
    /// rather than stripped, because silently renaming an index is how you
    /// describe the wrong one.
    #[test]
    fn identifiers_are_quoted_for_this_dialect() {
        assert_eq!(quote_ident("orders"), "\"orders\"");
        assert_eq!(quote_ident("odd\"name"), "\"odd\"\"name\"");
    }
}
