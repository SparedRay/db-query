//! The Elasticsearch engine against a real cluster.
//!
//! Everything about this engine is unit-tested against **recorded** payloads —
//! which proves the parsing and proves nothing about whether a cluster answers
//! the way the reference says. These close that gap.
//!
//!     mise run es-up
//!     mise run test-es
//!
//! Ignored by default; they need something listening on `ES_URL`
//! (default `http://localhost:9200`).

use db_query_lib::elastic::ElasticEngine;
use db_query_lib::engine::RowCap;
use db_query_lib::httpsql::Auth;

fn engine() -> ElasticEngine {
    let base = std::env::var("ES_URL").unwrap_or_else(|_| "http://localhost:9200".into());
    ElasticEngine::new(&base, Auth::None, None)
}

#[tokio::test]
#[ignore]
async fn the_cluster_reports_its_version() {
    let v = engine().version().await.expect("is the fixture up?");
    assert!(!v.is_empty());
    println!("elasticsearch {v}");
}

#[tokio::test]
#[ignore]
async fn a_select_returns_the_columns_and_rows_the_grid_expects() {
    let page = engine()
        .query(
            "SELECT user, total FROM orders ORDER BY total DESC",
            100,
            None,
        )
        .await
        .expect("query failed");

    assert_eq!(page.columns.len(), 2, "two projected columns");
    assert_eq!(page.rows.len(), 3, "the fixture seeds three documents");

    let outcome = db_query_lib::elastic::page_to_outcome(&page);
    match outcome {
        db_query_lib::exec::Outcome::Rows {
            columns,
            rows,
            truncated,
        } => {
            assert_eq!(columns[0].name, "user");
            assert_eq!(columns[1].name, "total");
            assert!(!truncated, "three rows under a cap of 100 is not truncated");
            // Ordered, so the largest total is first — proving the SQL ran
            // rather than the documents merely coming back.
            assert_eq!(
                rows[0][0],
                db_query_lib::decode::CellValue::Text("ada".into())
            );
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

/// The row cap is a request field, not a rewritten statement — so asking for
/// fewer rows than exist must set `truncated` without touching the SQL.
#[tokio::test]
#[ignore]
async fn a_small_page_is_reported_as_truncated() {
    let page = engine()
        .query("SELECT user FROM orders", 2, None)
        .await
        .expect("query failed");

    assert_eq!(page.rows.len(), 2);
    assert!(
        page.cursor.is_some(),
        "a partial read must hand back a cursor"
    );
    match db_query_lib::elastic::page_to_outcome(&page) {
        db_query_lib::exec::Outcome::Rows { truncated, .. } => assert!(truncated),
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn show_tables_finds_the_seeded_index() {
    let page = engine()
        .query("SHOW TABLES", 100, None)
        .await
        .expect("failed");
    let tables = db_query_lib::elastic::tables_from_page(&page);
    assert!(
        tables.iter().any(|t| t.name == "orders"),
        "got {:?}",
        tables.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore]
async fn describe_finds_the_mapped_fields() {
    let page = engine()
        .query("DESCRIBE \"orders\"", 100, None)
        .await
        .expect("failed");
    let cols = db_query_lib::elastic::columns_from_page(&page);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"user"), "got {names:?}");
    assert!(names.contains(&"total"), "got {names:?}");
}

/// The capabilities are a claim about the engine. This is the claim being
/// checked against the engine itself rather than against the documentation.
#[tokio::test]
#[ignore]
async fn the_cluster_really_does_refuse_writes() {
    let err = engine()
        .query("DELETE FROM orders", 10, None)
        .await
        .expect_err("Elasticsearch SQL must refuse a DELETE");
    println!("cluster said: {err}");
    assert!(!err.is_empty());
}

#[tokio::test]
#[ignore]
async fn an_unknown_index_produces_the_clusters_own_reason() {
    let err = engine()
        .query("SELECT * FROM definitely_not_here", 10, None)
        .await
        .expect_err("an unknown index must fail");
    // The reason, not a page of JSON.
    assert!(err.to_lowercase().contains("unknown"), "got: {err}");
    assert!(!err.contains("root_cause"), "raw JSON leaked: {err}");
}

#[test]
fn the_declared_row_cap_matches_how_this_engine_actually_limits() {
    assert_eq!(
        db_query_lib::elastic::capabilities().row_cap,
        RowCap::ServerPageSize
    );
}

// ------------------------------------------------- through the whole app

use db_query_lib::session::{self, AppState, ConnProfile, EngineKind};

const C: &str = "es1";
const T: &str = "tab1";

fn profile() -> ConnProfile {
    ConnProfile {
        id: C.into(),
        name: "fixture".into(),
        colour: "#3b82f6".into(),
        host: String::new(),
        port: 0,
        user: String::new(),
        database: None,
        allow_invalid_certs: false,
        kind: EngineKind::Elasticsearch,
        url: std::env::var("ES_URL").unwrap_or_else(|_| "http://localhost:9200".into()),
        auth: Auth::None,
        no_password: false,
        read_only: false,
    }
}

/// **The test that proves the seam**, rather than the engine.
///
/// Everything here goes through the same `connect` / `open_tab` / `run_script`
/// the Tauri commands call. If the dispatch were wrong, this fails while every
/// engine-level test above still passes.
#[tokio::test]
#[ignore]
async fn a_cluster_connects_and_runs_a_script_through_the_ordinary_path() {
    let state = AppState::default();

    let info = session::connect(&state, profile(), String::new())
        .await
        .expect("connect failed — is the fixture up?");
    assert!(!info.server_version.is_empty());
    // The capabilities the UI will read come from the engine, not a guess.
    assert!(!info.capabilities.writes);
    assert_eq!(info.capabilities.engine, "elasticsearch");
    assert_eq!(info.capabilities.namespace_label, "catalog");

    session::open_tab(&state, C, T).await.unwrap();

    let result = db_query_lib::exec::run_script(
        &state,
        T,
        "SELECT user, total FROM orders ORDER BY total DESC",
        true, // auto-LIMIT on: this engine must ignore it, not rewrite the SQL
        None,
    )
    .await
    .expect("run_script failed");

    assert_eq!(result.statements.len(), 1);
    let stmt = &result.statements[0];
    assert!(
        stmt.effective_sql.is_none(),
        "this engine caps rows with a request field and must never rewrite the user's SQL"
    );
    match &stmt.outcome {
        db_query_lib::exec::Outcome::Rows { rows, columns, .. } => {
            assert_eq!(columns.len(), 2);
            assert_eq!(rows.len(), 3);
        }
        other => panic!("expected rows, got {other:?}"),
    }

    session::disconnect(&state, C).await.unwrap();
}

/// A write is refused **by us**, before the request is sent — from the declared
/// capabilities, and reported as an ordinary failed statement.
#[tokio::test]
#[ignore]
async fn a_write_is_refused_before_it_reaches_the_cluster() {
    let state = AppState::default();
    session::connect(&state, profile(), String::new())
        .await
        .expect("connect failed");
    session::open_tab(&state, C, T).await.unwrap();

    let result = db_query_lib::exec::run_script(&state, T, "DELETE FROM orders", false, None)
        .await
        .expect("the script itself should not error");

    assert_eq!(result.aborted_at, Some(0));
    match &result.statements[0].outcome {
        db_query_lib::exec::Outcome::Error { message } => {
            assert!(message.contains("read-only"), "{message}");
            assert!(message.contains("SELECT"), "{message}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }

    // And the documents are still there — nothing was sent.
    let page = engine()
        .query("SELECT user FROM orders", 10, None)
        .await
        .unwrap();
    assert_eq!(
        page.rows.len(),
        3,
        "the refusal must not have deleted anything"
    );

    session::disconnect(&state, C).await.unwrap();
}

/// The schema tree's path: indices as tables, their mapped fields as columns.
#[tokio::test]
#[ignore]
async fn the_schema_tree_sees_indices_and_their_fields() {
    let state = AppState::default();
    session::connect(&state, profile(), String::new())
        .await
        .expect("connect failed");

    let tables = db_query_lib::schema::list_tables(&state, C, "")
        .await
        .unwrap();
    assert!(tables.iter().any(|t| t.name == "orders"), "{tables:?}");

    let cols = db_query_lib::schema::list_columns(&state, C, "", "orders")
        .await
        .unwrap();
    assert!(cols.iter().any(|c| c.name == "total"), "{cols:?}");

    // No stored routines, reported as an empty list rather than an error — the
    // tree omits empty groups, so nothing above has to know why.
    let routines = db_query_lib::schema::list_routines(&state, C, "")
        .await
        .unwrap();
    assert!(routines.is_empty());

    session::disconnect(&state, C).await.unwrap();
}

/// The double-click action, end to end.
///
/// A generated snippet is only correct if the server will run it, and the
/// previous one — MySQL backticks, catalog-qualified — would not. Unit tests
/// pin the *shape* of the string; only the cluster can say it parses.
#[tokio::test]
#[ignore]
async fn the_browse_snippet_a_double_click_generates_actually_runs() {
    let state = AppState::default();
    session::connect(&state, profile(), String::new())
        .await
        .expect("connect failed — is the fixture up?");
    session::open_tab(&state, C, T).await.unwrap();

    let server = session::server(&state, C).await.unwrap();
    let sql = server.engine.select_snippet("", "orders", 2).unwrap();
    println!("generated: {sql}");

    let result = db_query_lib::exec::run_script(&state, T, &sql, true, None)
        .await
        .expect("run_script failed");

    assert_eq!(result.statements.len(), 1);
    match &result.statements[0].outcome {
        db_query_lib::exec::Outcome::Rows { rows, .. } => {
            assert_eq!(rows.len(), 2, "LIMIT 2 should return two documents");
        }
        other => panic!("the cluster rejected the generated snippet: {other:?}"),
    }

    session::disconnect(&state, C).await.unwrap();
}

/// The same warm-up, on an engine whose namespaces are not schemas.
///
/// Proves it is the trait being called and not MySQL introspection wearing a
/// different name: a cluster has no `information_schema`, and its columns come
/// from `DESCRIBE`.
#[tokio::test]
#[ignore]
async fn the_assistant_prompt_is_warmed_for_a_cluster_too() {
    let state = AppState::default();
    session::connect(&state, profile(), String::new())
        .await
        .expect("connect failed — is the fixture up?");

    let warmed = db_query_lib::schema::warm_for_assistant(
        &state,
        C,
        "",
        db_query_lib::schema::ASSISTANT_TABLE_BUDGET,
    )
    .await
    .expect("warm failed");
    assert!(warmed.tables >= 1, "no indices found: {warmed:?}");
    assert_eq!(warmed.detailed, warmed.tables);

    let server = session::server(&state, C).await.unwrap();
    let cache = server.schema_cache.lock().await;
    let rendered = db_query_lib::assistant::render_schema("", cache.get("").unwrap());
    assert!(!rendered.contains("columns not loaded"), "{rendered}");
    assert!(rendered.contains("orders"), "{rendered}");
    // The mapped fields, which is the whole point of warming.
    assert!(rendered.contains("total"), "{rendered}");
}
