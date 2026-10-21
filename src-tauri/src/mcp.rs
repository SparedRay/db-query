//! An MCP server, so another tool can read the schema you are connected to,
//! put a query in your editor, and write down what a table means.
//!
//! # There is no execute tool, and that is the design
//!
//! A database MCP server that runs SQL is a dangerous thing to leave listening
//! on a laptop: an agent that misreads a request can drop a table. This one can
//! only *propose*, which gives it the blast radius of a stranger showing you
//! some text. `advertises_exactly_the_five_tools` in the tests below asserts the
//! advertised list, so a sixth tool cannot arrive by accident.
//!
//! # The one tool that writes, and what it writes to
//!
//! `put_notes` (Stage 25 slice 5) is how an agent with the code in front of it
//! writes the dictionary nobody has time to write by hand. It is still a
//! proposal: it runs no SQL, touches no database, and **does not write the
//! file** — the notes land in the dictionary tab as text the person reads and
//! saves. Nothing here grows a fifth risk class; it grows a second kind of
//! text put in front of somebody.
//!
//! # Why Streamable HTTP and not stdio
//!
//! The specification says clients SHOULD prefer stdio. We cannot: in stdio the
//! **client launches the server as a subprocess**, and a subprocess of db-query
//! is a second copy of the app — no window, no connections, no editor. What a
//! client needs to reach is the app that is already running.
//!
//! # What makes a local listener acceptable
//!
//! Off by default; bound to loopback; `Origin` validated (the spec's MUST,
//! without which a page you visit can drive this through DNS rebinding); and a
//! bearer token that lives in the OS keychain like every other secret here.
//! [`gate`] is the one place the last two are decided, so they can be tested
//! without a socket.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::header::{HeaderMap, AUTHORIZATION, ORIGIN, WWW_AUTHENTICATE};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use crate::secrets::{self, Secret};
use crate::session::AppState;

/// The one path this server answers on. Anything else is a 404, so a stray
/// `GET /` reports "wrong URL" rather than a protocol error.
pub const ENDPOINT_PATH: &str = "/mcp";

/// Keychain account for the bearer token. Namespaced the same way the
/// assistant's provider keys are, so one store holds all of them legibly.
const TOKEN_ID: &str = "mcp:token";

/// Default listening port, from IANA's dynamic/private range (49152-65535) so
/// it cannot collide with a registered service. Configurable in Settings,
/// because the one thing a fixed port guarantees is that somebody's machine
/// already has it.
pub const DEFAULT_PORT: u16 = 49731;

/// The event `put_query` emits. Namespaced so the frontend listener cannot be
/// confused with anything the user's own tabs do.
pub const PUT_QUERY_EVENT: &str = "mcp://put-query";

/// The event `put_notes` emits.
pub const PUT_NOTES_EVENT: &str = "mcp://put-notes";

/// How long `put_notes` waits for the editor to say what it did.
///
/// **Why it waits at all**, where `put_query` does not: a tab that failed to
/// arrive is visible the moment somebody looks at the window, and a note that
/// failed to merge is not. The failure this stage exists to prevent is a note
/// nobody notices is missing, so the tool reports what actually happened —
/// including "the window never answered".
const EDITOR_REPLY: std::time::Duration = std::time::Duration::from_secs(10);

/// The most tables one `put_notes` may carry.
///
/// Not a safety limit — nothing here can damage anything — but a proposal of
/// four hundred tables is one nobody will read, which defeats the point of
/// proposing.
const MAX_TABLES: usize = 60;

/// The protocol revision this server negotiates as its preferred version.
///
/// Written down rather than left to `ProtocolVersion::default()`, because the
/// spec revises and a silent bump is exactly the change that makes a client
/// half-work. `latest_protocol_version_is_the_one_we_recorded` fails when
/// upgrading `rmcp` moves it.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

// ------------------------------------------------------------------- the token

/// The bearer token, minted on first use.
///
/// Lives in the OS keychain and nowhere else — never in `connections.json`,
/// never in localStorage, never in a log line. 32 bytes of OS entropy, base64
/// for a header that has to survive being copied into a JSON config by hand.
pub fn token() -> Result<String, String> {
    match secrets::load(TOKEN_ID).map_err(|e| e.0)? {
        Some(existing) if !existing.is_empty() => Ok(existing.expose().to_string()),
        _ => mint(),
    }
}

/// Throw the current token away and issue a new one. Every configured client
/// stops working until it is given the new value, which is the point.
pub fn regenerate() -> Result<String, String> {
    mint()
}

fn mint() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    secrets::store(TOKEN_ID, &Secret::new(value.clone())).map_err(|e| e.0)?;
    Ok(value)
}

// -------------------------------------------------------------------- the gate

/// Why a request was refused. Kept as an enum so the HTTP status and the
/// message are decided in one place and the tests can name the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// No `Authorization: Bearer` header at all.
    NoToken,
    /// A bearer token that is not this server's.
    BadToken,
    /// An `Origin` this server will not serve. A **browser** sends this; a
    /// real MCP client sends none, which is why absent is allowed.
    ForeignOrigin,
}

impl Denied {
    pub fn status(self) -> StatusCode {
        match self {
            Self::NoToken | Self::BadToken => StatusCode::UNAUTHORIZED,
            Self::ForeignOrigin => StatusCode::FORBIDDEN,
        }
    }

    /// Said plainly, because the person reading it is configuring a client and
    /// "401" alone sends them to the wrong file. Nothing here reveals the
    /// token or any part of it.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoToken => {
                "This MCP server needs a bearer token. Copy it from Settings -> \
                 Integrations into your client's Authorization header."
            }
            Self::BadToken => {
                "That bearer token is not this server's. Copy the current one from \
                 Settings -> Integrations; regenerating it invalidates the old one."
            }
            Self::ForeignOrigin => {
                "Refused: this server answers only local origins. A web page cannot \
                 drive it."
            }
        }
    }
}

/// Everything that decides whether a request is answered at all.
///
/// A free function over a `HeaderMap` on purpose: the interesting cases are
/// header shapes, and testing them through a socket would prove less while
/// costing more.
pub fn gate(headers: &HeaderMap, token: &str) -> Result<(), Denied> {
    // Origin first. A browser that should never have reached us gets the
    // clearer refusal, and gets it without a token comparison.
    if let Some(origin) = headers.get(ORIGIN) {
        let origin = origin.to_str().unwrap_or("");
        if !origin_is_local(origin) {
            return Err(Denied::ForeignOrigin);
        }
    }

    let Some(auth) = headers.get(AUTHORIZATION) else {
        return Err(Denied::NoToken);
    };
    let auth = auth.to_str().map_err(|_| Denied::BadToken)?;
    let presented = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .ok_or(Denied::NoToken)?
        .trim();

    if constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        Ok(())
    } else {
        Err(Denied::BadToken)
    }
}

/// Whether an `Origin` header names this machine.
///
/// Per RFC 6454 an origin is scheme + host + port, and `null` is the *opaque*
/// origin a sandboxed frame sends — it names nothing, so it is refused rather
/// than treated as absent.
pub fn origin_is_local(origin: &str) -> bool {
    let rest = match origin.split_once("://") {
        Some(("http", rest)) | Some(("https", rest)) => rest,
        _ => return false,
    };
    // Split host from port. An IPv6 literal keeps its brackets, and what
    // follows the closing bracket has to be a port or nothing — `[::1]x` is a
    // different host that a lazier split would have read as `[::1]`.
    let (host, port) = if rest.starts_with('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        (&rest[..=end], &rest[end + 1..])
    } else {
        match rest.split_once(':') {
            Some((h, p)) => (h, &rest[h.len()..h.len() + 1 + p.len()]),
            None => (rest, ""),
        }
    };
    // An origin has no path, no userinfo and no query; anything after the host
    // must be `:<digits>`.
    let port_ok = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !port_ok {
        return false;
    }

    match host {
        "localhost" | "[::1]" => true,
        // The whole 127.0.0.0/8 loopback block, not just 127.0.0.1.
        h => h
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback()),
    }
}

/// Compare without leaking, through timing, how much of the token was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------------------- the tools

/// Which connection an MCP client sees.
///
/// `session.rs` deliberately keeps **no** ambient "current connection": a
/// command that resolves its own target can be raced onto the wrong server by a
/// UI switch mid-query. This is not that rule being relaxed. Nothing that
/// *executes* anything reads this — only the three read-only tools below, and
/// `put_query`, which writes text into a tab. Every existing command still
/// names the connection it acts on.
///
/// The consequence is documented rather than hidden: switch connection and the
/// agent's view changes under it.
type Focus = Arc<Mutex<Option<String>>>;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DatabaseArgs {
    /// Which database (or catalog) to look in. Omit to use the connection's
    /// own default.
    #[serde(default)]
    pub database: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TableArgs {
    /// Which database (or catalog) the table is in. Omit to use the
    /// connection's own default.
    #[serde(default)]
    pub database: Option<String>,
    /// The table or view name.
    pub table: String,
}

/// What `put_notes` takes: one entry per table, each saying as much or as
/// little as the caller knows.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NotesArgs {
    /// Which database (or catalog) these tables are in. Omit to use the
    /// connection's own default.
    #[serde(default)]
    pub database: Option<String>,
    /// One entry per table. Entries for tables the dictionary has never
    /// mentioned are added; entries for tables it already describes fill in
    /// what is missing.
    pub tables: Vec<TableNotesArg>,
    /// Overwrite notes the user has already written. Off by default: their
    /// sentence outranks a generated one, and anything already written is
    /// reported back as `kept` rather than replaced.
    #[serde(default)]
    pub replace: bool,
}

/// What to say about one table.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TableNotesArg {
    /// The table or view name.
    pub table: String,
    /// What the table is, in a sentence or two — what one row means, and what
    /// it is *not*. Shorter than a paragraph; this is read beside the column
    /// list, not instead of it.
    #[serde(default)]
    pub note: Option<String>,
    /// How it joins, as an expression: `orders.customer_id = customers.id`.
    #[serde(default)]
    pub join: Option<String>,
    /// Column name → what that column means. Only the ones whose name does
    /// not already say it: a note per column doubles what a prompt costs.
    #[serde(default)]
    pub columns: Option<std::collections::BTreeMap<String, String>>,
}

impl From<TableNotesArg> for crate::dictionary::NoteEdit {
    fn from(arg: TableNotesArg) -> Self {
        Self {
            table: arg.table,
            note: arg.note,
            join: arg.join,
            columns: arg.columns.unwrap_or_default().into_iter().collect(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryArgs {
    /// The SQL to place in a new editor tab. It is **not executed**.
    pub sql: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Databases {
    connection: String,
    engine: String,
    /// What this engine calls the thing: MySQL says "database",
    /// Elasticsearch says "catalog".
    label: String,
    databases: Vec<String>,
    default_database: Option<String>,
}

/// One table, as this server reports it.
///
/// **Its own shape rather than `schema::TableRef` plus a field.** `TableRef`
/// is what the introspection cache holds, and a note is not introspection: it
/// comes from a file the user edits and goes stale for entirely different
/// reasons. Keeping them apart is what stops a dictionary edit from having to
/// invalidate a schema cache.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TableOut {
    name: String,
    /// "BASE TABLE" | "VIEW"
    kind: String,
    /// What the user's dictionary says this table is, when they have said.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Tables {
    database: String,
    tables: Vec<TableOut>,
    /// Set when this connection has a dictionary attached, so a client can
    /// tell "nobody has described these" from "this app has no notes to give".
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dictionary: bool,
}

/// One column, with whatever the dictionary says about it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ColumnOut {
    name: String,
    data_type: String,
    nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Columns {
    database: String,
    table: String,
    columns: Vec<ColumnOut>,
    /// The table's own note, and how the user says it joins. Beside the
    /// columns rather than inside them, because that is what they describe.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    join: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Placed {
    placed: bool,
    /// Said back to the model in the tool result, so it stops asking for
    /// output that will never come.
    note: String,
}

/// What the tools are called, in the order they are advertised.
pub const TOOL_NAMES: [&str; 5] = [
    "list_databases",
    "list_tables",
    "describe_table",
    "put_query",
    "put_notes",
];

/// The handler. Cloned per request by the transport, so everything in it is
/// shared rather than owned.
///
/// Generic over the Tauri runtime for one reason: it is what lets
/// `tests/mcp_http.rs` drive the real socket against a mock app, so the
/// refusals are proved rather than reasoned about.
pub struct Tools<R: Runtime> {
    app: AppHandle<R>,
    focus: Focus,
    /// Shared with [`McpState`], because the reply comes back through a Tauri
    /// command rather than through this handler.
    pending: Pending,
    /// Built once per handler rather than per call. `#[tool_handler]` reaches
    /// it by the `router` expression below.
    tool_router: ToolRouter<Self>,
}

/// Written out rather than derived: `#[derive(Clone)]` would demand
/// `R: Clone`, and a Tauri runtime is not. Every field here is already a
/// handle or a router, so cloning is cheap either way.
impl<R: Runtime> Clone for Tools<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            focus: self.focus.clone(),
            pending: self.pending.clone(),
            tool_router: self.tool_router.clone(),
        }
    }
}

/// The payload of [`PUT_QUERY_EVENT`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutQuery {
    pub sql: String,
    /// Which connection it was aimed at, so a tab that arrives after the user
    /// has switched can say so instead of quietly attaching to the wrong one.
    pub connection_id: String,
}

/// The payload of [`PUT_NOTES_EVENT`].
///
/// The notes travel **structured**, not as TOML: the text is composed where
/// the file's own grammar lives (`dictionary::merge`), against the buffer as
/// it stands this instant, which is the only version that can be merged into
/// without losing whatever somebody is in the middle of typing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutNotes {
    /// Which call this is the answer to. The editor sends it back.
    pub request: String,
    pub connection_id: String,
    pub database: String,
    pub replace: bool,
    pub tables: Vec<crate::dictionary::NoteEdit>,
}

/// What the editor did with a [`PutNotes`].
///
/// Comes **from** the frontend, which is why it deserializes: the merge
/// happens against the open buffer, so only the window knows the answer.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesOutcome {
    #[serde(default)]
    pub applied: Vec<crate::dictionary::Applied>,
    #[serde(default)]
    pub skipped: Vec<crate::dictionary::Skipped>,
    /// Set when the editor could not take them at all — a connection that has
    /// closed, a dictionary that no longer parses.
    #[serde(default)]
    pub problem: Option<String>,
}

/// What `put_notes` answers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotesPlaced {
    /// True when something changed in the editor. False with an empty
    /// `skipped` means the file already said everything that was sent.
    placed: bool,
    applied: Vec<crate::dictionary::Applied>,
    skipped: Vec<crate::dictionary::Skipped>,
    note: String,
}

/// `put_notes` calls waiting for the window to answer, by request id.
type Pending = Arc<Mutex<std::collections::HashMap<String, oneshot::Sender<NotesOutcome>>>>;

#[tool_router]
impl<R: Runtime> Tools<R> {
    pub fn new(app: AppHandle<R>, focus: Focus, pending: Pending) -> Self {
        Self {
            app,
            focus,
            pending,
            tool_router: Self::tool_router(),
        }
    }

    /// Which connection the user is looking at, or a refusal that says how to
    /// fix it. An MCP client cannot choose: it sees the open window.
    async fn connection(&self) -> Result<String, ErrorData> {
        self.focus.lock().await.clone().ok_or_else(|| {
            ErrorData::invalid_request(
                "db-query has no connection open. Connect in the app first; this server \
                 always reports the connection you are looking at."
                    .to_string(),
                None,
            )
        })
    }

    /// The dictionary attached to a connection, if any and if it loads.
    ///
    /// **Never an error.** A dictionary with a typo in it must not turn
    /// `list_tables` into a failure — the client asked about the schema, and
    /// the schema is still there. The app's own warning surface is the place
    /// that complains about the file.
    async fn dictionary(&self, id: &str) -> Option<crate::dictionary::Dictionary> {
        let state = self.app.state::<AppState>();
        let server = crate::session::server(&state, id).await.ok()?;
        let path = server
            .profile
            .dictionary
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())?;
        crate::dictionary::load_cached(std::path::Path::new(path))
            .ok()
            .map(|l| l.dictionary)
            .filter(|d| !d.is_empty())
    }

    /// Resolve `database`, falling back to the connection's own default.
    async fn database(&self, given: Option<String>, id: &str) -> Result<String, ErrorData> {
        if let Some(db) = given
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
        {
            return Ok(db);
        }
        let state = self.app.state::<AppState>();
        let server = crate::session::server(&state, id)
            .await
            .map_err(|e| ErrorData::invalid_request(e, None))?;
        server
            .profile
            .database
            .clone()
            .filter(|d| !d.is_empty())
            .ok_or_else(|| {
                ErrorData::invalid_request(
                    no_default_namespace(&server.capabilities.namespace_label),
                    None,
                )
            })
    }

    /// The databases on the connection the app currently has in front of the
    /// user. Read-only.
    #[tool(
        description = "List the databases (or catalogs) on the db-query connection the user \
                       currently has open. Read-only."
    )]
    async fn list_databases(&self) -> Result<CallToolResult, ErrorData> {
        let id = self.connection().await?;
        let state = self.app.state::<AppState>();
        let server = crate::session::server(&state, &id)
            .await
            .map_err(|e| ErrorData::invalid_request(e, None))?;
        let databases = server
            .engine
            .namespaces(&server)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        structured(&Databases {
            connection: server.profile.name.clone(),
            engine: server.capabilities.engine.clone(),
            label: server.capabilities.namespace_label.clone(),
            databases,
            default_database: server.profile.database.clone().filter(|d| !d.is_empty()),
        })
    }

    /// Tables and views in one database. Read-only.
    #[tool(
        description = "List the tables and views in one database on the db-query connection \
                       the user currently has open. Each may carry a `note`: what the user's \
                       own dictionary says that table is. Read-only."
    )]
    async fn list_tables(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = self.connection().await?;
        let db = self.database(args.database, &id).await?;
        let tables = {
            let state = self.app.state::<AppState>();
            crate::schema::list_tables(&state, &id, &db)
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?
        };
        let dictionary = self.dictionary(&id).await;
        structured(&Tables {
            tables: tables_with_notes(&db, tables, dictionary.as_ref()),
            database: db,
            dictionary: dictionary.is_some(),
        })
    }

    /// One table's columns, with types, nullability and keys. Read-only, and
    /// **no rows** — row data is a larger privacy question than schema names
    /// and is deliberately not offered here.
    #[tool(
        description = "Describe one table or view: its columns, types, nullability and keys, \
                       plus any `note` the user's own dictionary holds for the table, for how \
                       it joins, or for a column. Those notes are the user's and outrank a \
                       guess at what a name means. Returns no row data. Read-only."
    )]
    async fn describe_table(
        &self,
        Parameters(args): Parameters<TableArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = self.connection().await?;
        let db = self.database(args.database, &id).await?;
        let columns = {
            let state = self.app.state::<AppState>();
            crate::schema::list_columns(&state, &id, &db, &args.table)
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?
        };
        let said = self
            .dictionary(&id)
            .await
            .and_then(|d| d.table(&db, &args.table));
        structured(&Columns {
            columns: columns_with_notes(columns, said.as_ref()),
            note: said.as_ref().and_then(|e| e.note.clone()),
            join: said.as_ref().and_then(|e| e.join.clone()),
            database: db,
            table: args.table,
        })
    }

    /// Put SQL in a new tab. **Never runs it** — this app's oldest rule is
    /// that the person at the keyboard presses run.
    #[tool(
        description = "Open a new editor tab in db-query containing this SQL. The query is \
                       NOT executed and no results are returned — the user reviews it and \
                       runs it themselves. Use this to hand a query over, never to read data."
    )]
    async fn put_query(
        &self,
        Parameters(args): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = self.connection().await?;
        let sql = args.sql.trim().to_string();
        if sql.is_empty() {
            return Err(ErrorData::invalid_params(
                "The SQL was empty.".to_string(),
                None,
            ));
        }

        self.app
            .emit(
                PUT_QUERY_EVENT,
                PutQuery {
                    sql,
                    connection_id: id,
                },
            )
            .map_err(|e| {
                ErrorData::internal_error(format!("Could not reach the editor: {e}"), None)
            })?;

        structured(&Placed {
            placed: true,
            note: "The query is open in a new tab, marked as external. It has not been run, \
                   and this server cannot run it."
                .to_string(),
        })
    }

    /// Write what tables mean into the user's dictionary — **into the editor**,
    /// never into the file.
    ///
    /// The tool this stage was asked for: an agent that has the code in front
    /// of it knows what `status` means, and nobody has time to write three
    /// hundred notes by hand. What makes it acceptable is that it ends where
    /// `put_query` ends — as text in a tab, waiting for somebody to read it
    /// and press save.
    #[tool(
        description = "Write notes into the user's AI dictionary for the db-query connection \
                       they have open: what a table is, how it joins, and what its confusing \
                       columns mean. The notes are merged into the dictionary file IN THE \
                       EDITOR and are NOT saved to disk — the user reviews them and saves. \
                       Notes the user has already written are kept, not overwritten, unless \
                       `replace` is true; the result says which were written and which were \
                       kept. Writes nothing to the database and runs no SQL."
    )]
    async fn put_notes(
        &self,
        Parameters(args): Parameters<NotesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = self.connection().await?;
        let database = self.database(args.database, &id).await?;

        // Where somebody's dictionary lives is their decision, and this server
        // does not get to make it: an unbound connection is told how, not
        // given a file it did not ask for.
        let name = {
            let state = self.app.state::<AppState>();
            let server = crate::session::server(&state, &id)
                .await
                .map_err(|e| ErrorData::invalid_request(e, None))?;
            let bound = server
                .profile
                .dictionary
                .as_deref()
                .map(str::trim)
                .is_some_and(|p| !p.is_empty());
            if !bound {
                return Err(ErrorData::invalid_request(
                    format!(
                        "“{}” has no AI dictionary attached, so there is nowhere to write \
                         these notes. In db-query: right-click the connection in the rail \
                         and choose “Add an AI dictionary…”.",
                        server.profile.name
                    ),
                    None,
                ));
            }
            server.profile.name.clone()
        };

        if args.tables.is_empty() {
            return Err(ErrorData::invalid_params(
                "No tables were given.".to_string(),
                None,
            ));
        }
        if args.tables.len() > MAX_TABLES {
            return Err(ErrorData::invalid_params(
                format!(
                    "{} tables in one call; {MAX_TABLES} is the limit. These are a proposal \
                     somebody has to read before it is saved, so send them in batches.",
                    args.tables.len()
                ),
                None,
            ));
        }

        let tables: Vec<crate::dictionary::NoteEdit> =
            args.tables.into_iter().map(Into::into).collect();

        let request = request_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request.clone(), tx);

        let emitted = self.app.emit(
            PUT_NOTES_EVENT,
            PutNotes {
                request: request.clone(),
                connection_id: id,
                database,
                replace: args.replace,
                tables,
            },
        );
        if let Err(e) = emitted {
            self.pending.lock().await.remove(&request);
            return Err(ErrorData::internal_error(
                format!("Could not reach the editor: {e}"),
                None,
            ));
        }

        let outcome = match tokio::time::timeout(EDITOR_REPLY, rx).await {
            Ok(Ok(outcome)) => outcome,
            // Either the window never answered, or it answered and went away
            // mid-sentence. Both mean the same thing to a client: nothing is
            // known to have landed, so do not say it did.
            _ => {
                self.pending.lock().await.remove(&request);
                return Err(ErrorData::internal_error(
                    "db-query did not take the notes within ten seconds. The window may be \
                     busy or closed; nothing was written."
                        .to_string(),
                    None,
                ));
            }
        };

        if let Some(problem) = outcome.problem {
            return Err(ErrorData::invalid_request(problem, None));
        }

        let changed = outcome
            .applied
            .iter()
            .filter(|a| !a.wrote.is_empty())
            .count();
        let kept = outcome.applied.iter().any(|a| !a.kept.is_empty());

        structured(&NotesPlaced {
            placed: changed > 0,
            note: placed_note(&name, changed, kept),
            applied: outcome.applied,
            skipped: outcome.skipped,
        })
    }
}

/// What `put_notes` says it did.
///
/// A free function for the reason the three above are: the sentence a client
/// is told is the part worth pinning, and it must never read as "saved". A
/// model that believes the file is written stops telling the user to look at
/// it.
fn placed_note(connection: &str, changed: usize, kept: bool) -> String {
    let mut note = if changed == 0 {
        format!(
            "Nothing changed: the dictionary for “{connection}” already said everything \
             that was sent."
        )
    } else {
        format!(
            "{changed} table{} written into the dictionary tab in db-query. It is NOT saved \
             — the user reviews it and saves, and this server cannot save it for them.",
            if changed == 1 { "" } else { "s" }
        )
    };
    if kept {
        note.push_str(
            " Some notes the user had already written were kept; send `replace: true` only \
             if you mean to overwrite their own words.",
        );
    }
    note
}

/// An id for one `put_notes`, so the window's answer finds the call that is
/// waiting for it. Random rather than a counter: two windows of a future
/// multi-window build would both count from one.
fn request_id() -> String {
    let mut bytes = [0u8; 12];
    rand::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Attach the user's notes to a table list.
///
/// **A free function, for the reason the two below are**: everything else in
/// this module needs an app handle and a live connection, and the thing most
/// worth pinning here is the shape a client receives.
fn tables_with_notes(
    database: &str,
    tables: Vec<crate::schema::TableRef>,
    dictionary: Option<&crate::dictionary::Dictionary>,
) -> Vec<TableOut> {
    tables
        .into_iter()
        .map(|t| TableOut {
            note: dictionary
                .and_then(|d| d.table(database, &t.name))
                .and_then(|e| e.note),
            name: t.name,
            kind: t.kind,
        })
        .collect()
}

/// Attach the user's notes to one table's columns.
///
/// `said` is that table's whole entry; the column keys in it are folded to
/// lower case by the dictionary, and a server may report a column in any case
/// at all — which is why the lookup folds rather than compares.
fn columns_with_notes(
    columns: Vec<crate::schema::ColumnInfo>,
    said: Option<&crate::dictionary::TableNote>,
) -> Vec<ColumnOut> {
    columns
        .into_iter()
        .map(|c| ColumnOut {
            note: said
                .and_then(|e| e.columns.get(&c.name.to_lowercase()))
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            name: c.name,
            data_type: c.data_type,
            nullable: c.nullable,
            key: c.key,
        })
        .collect()
}

/// What to say when the caller named no namespace and the connection has no
/// default one.
///
/// **In the engine's own word.** `namespace_label` exists precisely so MySQL's
/// vocabulary is not put in front of every engine, and Elasticsearch — which
/// says "catalog" and has no default one — is where that shows: this is the
/// message its users see on the first call every time.
///
/// A free function so the property is testable without a live connection.
fn no_default_namespace(label: &str) -> String {
    format!(
        "This connection has no default {label}. Call list_databases and pass one \
         as `database`."
    )
}

fn structured<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    serde_json::to_value(value)
        .map(CallToolResult::structured)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

#[tool_handler(router = self.tool_router)]
impl<R: Runtime> ServerHandler for Tools<R> {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.protocol_version = ProtocolVersion::LATEST;
        info.server_info =
            Implementation::new("db-query", env!("CARGO_PKG_VERSION")).with_title("db-query");
        info.instructions = Some(
            "Read-only access to the schema of the database connection the user has open \
             in db-query, plus put_query, which places SQL in a new editor tab without \
             running it. There is no way to execute a statement or read rows through this \
             server: propose the query and let the user run it.\n\n\
             Where a table or column carries a `note`, that is the user's own description \
             of it, from a dictionary they maintain beside the schema. Prefer it over what \
             a name appears to mean; a table with no note is not described anywhere, so ask \
             rather than inferring.\n\n\
             put_notes writes that dictionary. When you have the application's code or its \
             migrations in front of you and know what a table means, say so with put_notes \
             rather than leaving it in this conversation: it lands in the user's editor for \
             them to review and save, and every later session reads it. It saves nothing \
             itself, and it keeps whatever the user has already written."
                .into(),
        );
        info
    }
}

// ------------------------------------------------------------- the server state

/// The listener, and the connection an MCP client sees.
#[derive(Default)]
pub struct McpState {
    running: Mutex<Option<Running>>,
    focus: Focus,
    pending: Pending,
}

struct Running {
    port: u16,
    /// Cancels the accept loop **and** every connection it spawned. Without the
    /// second half, "off" would mean "off for the next client".
    stop: CancellationToken,
    /// The accept loop, so [`stop`] can **wait** for it rather than merely ask.
    ///
    /// The listener is owned by that task, so the port is held until the task
    /// ends. Cancelling the token only schedules that; returning before it has
    /// happened is what made restarting on the same port fail with "address
    /// already in use" — see `stop_releases_the_port_before_it_returns`.
    accept: tokio::task::JoinHandle<()>,
}

/// What Settings shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
}

impl McpState {
    /// Tell the server which connection to report. Called by the frontend when
    /// the user switches; `None` when nothing is open.
    pub async fn set_focus(&self, id: Option<String>) {
        *self.focus.lock().await = id;
    }

    /// The editor's answer to a [`PUT_NOTES_EVENT`].
    ///
    /// An answer to a request nobody is waiting for is **dropped silently**:
    /// the tool has already given up and told the client so, and a second
    /// opinion arriving late has nowhere to go.
    pub async fn notes_landed(&self, request: String, outcome: NotesOutcome) {
        if let Some(tx) = self.pending.lock().await.remove(&request) {
            let _ = tx.send(outcome);
        }
    }

    pub async fn status(&self) -> McpStatus {
        match self.running.lock().await.as_ref() {
            Some(r) => McpStatus {
                running: true,
                port: Some(r.port),
                url: Some(url(r.port)),
            },
            None => McpStatus {
                running: false,
                port: None,
                url: None,
            },
        }
    }
}

pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{ENDPOINT_PATH}")
}

/// Start listening. Idempotent in the sense that matters: an already-running
/// server is stopped first, so changing the port in Settings does not leave the
/// old one open.
///
/// The bind happens **before** the accept loop is spawned, so "that port is
/// already in use" is reported to the UI rather than logged into the void.
pub async fn start<R: Runtime>(
    app: &AppHandle<R>,
    state: &McpState,
    port: u16,
) -> Result<McpStatus, String> {
    // Read the token *before* binding: a server listening without one is the
    // failure this whole module exists to avoid, and a machine with no
    // credential store should refuse to start rather than start open.
    let token = token()?;
    start_with_token(app, state, port, token).await
}

/// [`start`], with the token supplied rather than read.
///
/// The split exists for `tests/mcp_http.rs`: driving the real socket must not
/// need a keychain, or the one test that proves the door is locked would be
/// the one test CI cannot run.
pub async fn start_with_token<R: Runtime>(
    app: &AppHandle<R>,
    state: &McpState,
    port: u16,
    token: String,
) -> Result<McpStatus, String> {
    stop(state).await;

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Could not listen on 127.0.0.1:{port} ({e}). Choose another port."))?;
    // Port 0 means "the OS picks", which the tests use; report what it picked.
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    let stop_token = CancellationToken::new();
    let tools = Tools::new(app.clone(), state.focus.clone(), state.pending.clone());

    // Belt and braces: `gate` refuses a foreign Origin before rmcp sees the
    // request, and rmcp refuses it again. rmcp's default here is an **empty
    // list, which means no validation at all**, so leaving it alone would be a
    // silent downgrade of the spec's one MUST.
    let config = StreamableHttpServerConfig::default()
        .with_allowed_origins(local_origins(bound))
        .with_allowed_hosts(["localhost", "127.0.0.1", "::1"])
        .with_cancellation_token(stop_token.clone());
    let service = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let accept_stop = stop_token.clone();
    let accept = tokio::spawn(async move {
        loop {
            let stream = tokio::select! {
                _ = accept_stop.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => stream,
                    // One failed accept is not a reason to stop listening.
                    Err(_) => continue,
                },
            };

            let service = service.clone();
            let token = token.clone();
            let conn_stop = accept_stop.clone();
            tokio::spawn(async move {
                let handler =
                    hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| {
                        let service = service.clone();
                        let token = token.clone();
                        async move { Ok::<_, Infallible>(respond(service, &token, req).await) }
                    });
                let conn = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), handler);
                tokio::pin!(conn);
                tokio::select! {
                    _ = conn.as_mut() => {}
                    _ = conn_stop.cancelled() => {}
                }
            });
        }
    });

    *state.running.lock().await = Some(Running {
        port: bound,
        stop: stop_token,
        accept,
    });

    Ok(McpStatus {
        running: true,
        port: Some(bound),
        url: Some(url(bound)),
    })
}

/// Stop listening, and **do not return until the port is free**.
///
/// The token cancels the accept loop and every connection it spawned; awaiting
/// the accept task is what turns that from a request into a fact. The listener
/// lives inside that task, so until it ends the socket is still bound — and
/// `start_with_token` stops before it binds, which means anything less than
/// waiting here makes restarting on the same port a race with itself.
///
/// That race was real and was reported from use: regenerating the token, which
/// restarts on the port already in use, reported "another app is using the
/// port". The other app was this one.
pub async fn stop(state: &McpState) {
    let running = state.running.lock().await.take();
    if let Some(running) = running {
        running.stop.cancel();
        // A panicked accept loop is still a stopped one, and there is nothing
        // useful to do about it here: the port is free either way.
        let _ = running.accept.await;
    }
}

/// The origins a page on this machine could legitimately have. `null` is not
/// among them: it is the opaque origin a sandboxed frame sends, and it names
/// nobody.
fn local_origins(port: u16) -> Vec<String> {
    let mut origins = Vec::new();
    for host in ["localhost", "127.0.0.1", "[::1]"] {
        origins.push(format!("http://{host}:{port}"));
        origins.push(format!("http://{host}"));
        origins.push(format!("https://{host}"));
    }
    origins
}

type Body = BoxBody<Bytes, Infallible>;

async fn respond<R: Runtime>(
    service: StreamableHttpService<Tools<R>, LocalSessionManager>,
    token: &str,
    req: Request<hyper::body::Incoming>,
) -> Response<Body> {
    if req.uri().path() != ENDPOINT_PATH {
        return text(
            StatusCode::NOT_FOUND,
            format!("db-query's MCP server is at {ENDPOINT_PATH}."),
            None,
        );
    }
    if let Err(denied) = gate(req.headers(), token) {
        return text(
            denied.status(),
            denied.message().to_string(),
            matches!(denied, Denied::NoToken | Denied::BadToken).then_some("Bearer"),
        );
    }
    service.handle(req).await
}

fn text(status: StatusCode, body: String, challenge: Option<&str>) -> Response<Body> {
    let mut builder = Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8");
    if let Some(challenge) = challenge {
        builder = builder.header(WWW_AUTHENTICATE, challenge);
    }
    builder
        .body(Full::new(Bytes::from(body)).boxed())
        .expect("a static response cannot fail to build")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                hyper::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    // --------------------------------------- Stage 25: the user's own notes

    fn dict() -> crate::dictionary::Dictionary {
        crate::dictionary::parse(
            r#"
            [tables.orders]
            note = "One row per order."
            join = "orders.user_id = users.id"
            columns.total = "Cents. Divide by 100 only for a person."
            columns.blank = "   "
            "#,
        )
        .unwrap()
        .dictionary
    }

    fn table(name: &str) -> crate::schema::TableRef {
        crate::schema::TableRef {
            name: name.into(),
            kind: "BASE TABLE".into(),
        }
    }

    fn column(name: &str) -> crate::schema::ColumnInfo {
        crate::schema::ColumnInfo {
            name: name.into(),
            data_type: "int".into(),
            nullable: false,
            key: None,
        }
    }

    /// A described table carries its note; one nobody wrote about carries
    /// **no key at all**, rather than `null` — a client should see the absence,
    /// not a field it has to interpret.
    #[test]
    fn a_table_list_carries_the_notes_and_omits_the_rest() {
        let d = dict();
        let out = tables_with_notes("poc", vec![table("orders"), table("big")], Some(&d));
        let json = serde_json::to_value(&out).unwrap();

        assert_eq!(json[0]["name"], "orders");
        assert_eq!(json[0]["note"], "One row per order.");
        assert_eq!(json[1]["name"], "big");
        assert!(
            json[1].get("note").is_none(),
            "an undescribed table must carry no note key: {json}"
        );
    }

    /// With no dictionary at all, the list is what it always was.
    #[test]
    fn with_no_dictionary_the_list_is_unchanged() {
        let out = tables_with_notes("poc", vec![table("orders")], None);
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json[0]["name"], "orders");
        assert_eq!(json[0]["kind"], "BASE TABLE");
        assert!(json[0].get("note").is_none(), "{json}");
    }

    /// Columns the same way, and the case the server reports does not hide a
    /// note — the dictionary is keyed lower-case and MySQL is not.
    #[test]
    fn a_column_note_survives_the_case_the_server_reports() {
        let d = dict();
        let said = d.table("poc", "orders");
        let out = columns_with_notes(
            vec![column("id"), column("TOTAL"), column("blank")],
            said.as_ref(),
        );
        let json = serde_json::to_value(&out).unwrap();

        assert!(json[0].get("note").is_none(), "id has none: {json}");
        assert_eq!(json[1]["note"], "Cents. Divide by 100 only for a person.");
        assert!(
            json[2].get("note").is_none(),
            "a note of only spaces is not a note: {json}"
        );
    }

    /// The table's own note and its join travel **beside** the columns, which
    /// is what they describe — and `dictionary` says whether this connection
    /// has one at all, so a client can tell "nobody described these" from
    /// "this app has nothing to give".
    #[test]
    fn the_described_shapes_serialise_as_a_client_reads_them() {
        let d = dict();
        let said = d.table("poc", "orders");
        let columns = Columns {
            database: "poc".into(),
            table: "orders".into(),
            columns: columns_with_notes(vec![column("total")], said.as_ref()),
            note: said.as_ref().and_then(|e| e.note.clone()),
            join: said.as_ref().and_then(|e| e.join.clone()),
        };
        let json = serde_json::to_value(&columns).unwrap();
        assert_eq!(json["note"], "One row per order.");
        assert_eq!(json["join"], "orders.user_id = users.id");
        assert_eq!(
            json["columns"][0]["note"],
            "Cents. Divide by 100 only for a person."
        );

        let bare = Columns {
            database: "poc".into(),
            table: "big".into(),
            columns: columns_with_notes(vec![column("id")], None),
            note: None,
            join: None,
        };
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("note").is_none(), "{json}");
        assert!(json.get("join").is_none(), "{json}");

        let listed = Tables {
            database: "poc".into(),
            tables: tables_with_notes("poc", vec![table("big")], None),
            dictionary: false,
        };
        let json = serde_json::to_value(&listed).unwrap();
        assert!(
            json.get("dictionary").is_none(),
            "absent rather than false, like every other optional here: {json}"
        );
    }

    // ------------------------------------- Stage 25 slice 5: put_notes

    /// **The sentence that matters.** A client told "saved" stops telling the
    /// user to look at their editor, and the file is never saved here.
    #[test]
    fn the_report_never_reads_as_saved() {
        let one = placed_note("prod-eu", 1, false);
        assert!(one.contains("1 table written"), "{one}");
        assert!(one.contains("NOT saved"), "{one}");
        assert!(one.contains("the user reviews it and saves"), "{one}");

        let many = placed_note("prod-eu", 4, false);
        assert!(many.contains("4 tables written"), "{many}");

        let none = placed_note("prod-eu", 0, false);
        assert!(none.contains("Nothing changed"), "{none}");
        assert!(!none.contains("written into"), "{none}");

        let kept = placed_note("prod-eu", 2, true);
        assert!(kept.contains("were kept"), "{kept}");
        assert!(kept.contains("replace: true"), "{kept}");
        assert!(
            !placed_note("prod-eu", 2, false).contains("were kept"),
            "nothing kept, nothing said about keeping"
        );
    }

    /// What an agent sends becomes one edit per table, and an omitted
    /// `columns` is an empty map rather than a failure to deserialize.
    #[test]
    fn an_agents_table_becomes_one_edit() {
        let arg: TableNotesArg = serde_json::from_value(serde_json::json!({
            "table": "Orders",
            "note": "One row per order.",
            "columns": { "status": "P, S or X." }
        }))
        .expect("the shape a client sends");
        let edit: crate::dictionary::NoteEdit = arg.into();
        assert_eq!(edit.table, "Orders", "the case is kept; the merge folds it");
        assert_eq!(edit.note.as_deref(), Some("One row per order."));
        assert_eq!(edit.join, None);
        assert_eq!(
            edit.columns.get("status").map(String::as_str),
            Some("P, S or X.")
        );

        let bare: TableNotesArg =
            serde_json::from_value(serde_json::json!({ "table": "audit_log" })).unwrap();
        let edit: crate::dictionary::NoteEdit = bare.into();
        assert!(edit.columns.is_empty());
        assert!(edit.note.is_none() && edit.join.is_none());
    }

    /// The reply crosses a process boundary — the frontend sends it as JSON —
    /// so the one thing worth pinning is that **its shape is the shape the
    /// window sends**, camelCase and all, with every field optional.
    #[test]
    fn the_editors_reply_deserializes_as_the_window_sends_it() {
        let outcome: NotesOutcome = serde_json::from_value(serde_json::json!({
            "applied": [{ "table": "orders", "wrote": ["note"], "kept": ["join"] }],
            "skipped": [{ "table": "oders", "why": "Nothing to write." }]
        }))
        .expect("what main.ts sends");
        assert_eq!(outcome.applied[0].table, "orders");
        assert_eq!(outcome.applied[0].wrote, vec!["note"]);
        assert_eq!(outcome.applied[0].kept, vec!["join"]);
        assert_eq!(outcome.skipped[0].table, "oders");
        assert!(outcome.problem.is_none());

        // The failure case, which is the only one that sets `problem`.
        let outcome: NotesOutcome =
            serde_json::from_value(serde_json::json!({ "problem": "That connection is closed." }))
                .expect("a refusal is a reply too");
        assert_eq!(
            outcome.problem.as_deref(),
            Some("That connection is closed.")
        );
        assert!(outcome.applied.is_empty() && outcome.skipped.is_empty());
    }

    /// Two calls in flight must not answer each other.
    #[test]
    fn two_requests_do_not_share_an_id() {
        let ids: std::collections::BTreeSet<String> = (0..64).map(|_| request_id()).collect();
        assert_eq!(ids.len(), 64);
    }

    /// The runtime is irrelevant to the tool list — `Wry` is named only
    /// because `Tools` is generic. See `tests/mcp_http.rs` for the same
    /// assertion against a mock app over a real socket.
    ///
    /// The whole point of the module: the advertised list is these five and
    /// nothing else. A `run_query` added in a hurry stops the build here.
    ///
    /// Slice 5 added the fifth on purpose, and this is the test that had to be
    /// renamed and extended by hand to let it through — which is why the list
    /// is asserted rather than the length. (It had drifted away from its test
    /// when slice 3 inserted a block beneath it; clippy noticed, which is more
    /// than the compiler did.)
    #[test]
    fn advertises_exactly_the_five_tools() {
        let names: Vec<String> = Tools::<tauri::Wry>::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        let mut expected: Vec<String> = TOOL_NAMES.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(sorted, expected, "advertised tools changed: {names:?}");
    }

    /// Stated separately from the list above, because this is the property a
    /// reader of the tracker is checking for.
    #[test]
    fn nothing_advertised_can_execute() {
        for tool in Tools::<tauri::Wry>::tool_router().list_all() {
            let name = tool.name.to_string();
            assert!(
                !name.contains("run") && !name.contains("exec") && !name.contains("query_result"),
                "{name} sounds like it executes"
            );
        }
    }

    #[test]
    fn a_request_with_no_token_is_refused() {
        assert_eq!(gate(&headers(&[]), "secret"), Err(Denied::NoToken));
        assert_eq!(
            gate(&headers(&[("authorization", "Basic abc")]), "secret"),
            Err(Denied::NoToken)
        );
    }

    #[test]
    fn a_wrong_token_is_refused() {
        assert_eq!(
            gate(&headers(&[("authorization", "Bearer nope")]), "secret"),
            Err(Denied::BadToken)
        );
        // A prefix of the real token is still wrong, which is the case a
        // length-only comparison would let through.
        assert_eq!(
            gate(&headers(&[("authorization", "Bearer secre")]), "secret"),
            Err(Denied::BadToken)
        );
    }

    #[test]
    fn the_right_token_is_accepted_either_way_it_is_spelled() {
        assert_eq!(
            gate(&headers(&[("authorization", "Bearer secret")]), "secret"),
            Ok(())
        );
        assert_eq!(
            gate(&headers(&[("authorization", "bearer secret")]), "secret"),
            Ok(())
        );
    }

    /// A real MCP client is not a browser and sends no `Origin`. Refusing that
    /// would refuse every client there is.
    #[test]
    fn an_absent_origin_is_allowed() {
        assert_eq!(
            gate(&headers(&[("authorization", "Bearer secret")]), "secret"),
            Ok(())
        );
    }

    #[test]
    fn a_foreign_origin_is_refused_before_the_token_is_even_checked() {
        let h = headers(&[
            ("origin", "https://evil.example"),
            ("authorization", "Bearer secret"),
        ]);
        assert_eq!(gate(&h, "secret"), Err(Denied::ForeignOrigin));
    }

    #[test]
    fn loopback_origins_are_allowed_and_lookalikes_are_not() {
        for good in [
            "http://localhost",
            "http://localhost:5173",
            "http://127.0.0.1:49731",
            "http://127.0.0.2",
            "https://localhost:8080",
            "http://[::1]:49731",
        ] {
            assert!(origin_is_local(good), "{good} should be local");
        }
        for bad in [
            // The classic DNS-rebinding lookalikes.
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://evil.example",
            // The opaque origin a sandboxed frame sends. It names nobody.
            "null",
            "",
            // Not an HTTP origin at all.
            "file://",
            // A host that merely starts like a loopback literal.
            "http://[::1]x",
            "http://[::1]evil.example",
            // Userinfo and path, both of which an origin does not have.
            "http://127.0.0.1@evil.example",
            "http://localhost/../evil",
            "http://localhost:80x",
        ] {
            assert!(!origin_is_local(bad), "{bad} should not be local");
        }
    }

    /// The engine's word, not MySQL's. Elasticsearch is the case that made this
    /// visible: it calls them catalogs and has no default one, so this message
    /// is the first thing an MCP client sees on that connection.
    #[test]
    fn the_missing_namespace_message_uses_the_engines_own_word() {
        assert!(no_default_namespace("catalog").contains("no default catalog"));
        assert!(no_default_namespace("database").contains("no default database"));
        // And it says which argument to pass, since the argument is called
        // `database` whatever the engine calls the thing.
        assert!(no_default_namespace("catalog").contains("`database`"));
        assert!(
            !no_default_namespace("catalog").contains("no default database"),
            "MySQL's word leaked into another engine's message"
        );
    }

    #[test]
    fn constant_time_eq_still_compares_correctly() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    /// The protocol revision is a fact about interoperability, so a change to
    /// it should be a decision rather than a side effect of `cargo update`.
    #[test]
    fn latest_protocol_version_is_the_one_we_recorded() {
        assert_eq!(
            ProtocolVersion::LATEST.as_str(),
            PROTOCOL_VERSION,
            "rmcp's preferred protocol version moved; check what clients negotiate \
             before changing this"
        );
    }

    #[test]
    fn the_advertised_origins_cover_the_bound_port_only() {
        let origins = local_origins(49731);
        assert!(origins.contains(&"http://127.0.0.1:49731".to_string()));
        assert!(!origins.iter().any(|o| o.contains("evil")));
        assert!(!origins.iter().any(|o| o == "null"));
    }
}
