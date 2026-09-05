//! Milestone checks against a live MySQL server.
//!
//! Ignored by default so `cargo test` stays offline-friendly. Bring the
//! fixture up first (see dev/README.md), then:
//!
//!     cargo test --test live_mysql -- --ignored --test-threads=1
//!
//! Single-threaded on purpose: every test shares one AppState with one exec
//! connection, and MySQL session state (USE, temp tables) is per-connection.

use db_query_lib::decode::CellValue;
use db_query_lib::exec::{self, Outcome, StatementKind};
use db_query_lib::schema;
use db_query_lib::session::{self, AppState, ConnProfile};
use sqlx::Row;
use std::sync::atomic::Ordering;

/// Every test that does not care about tab identity uses this one.
const T: &str = "t1";
/// Likewise for connection identity.
const C: &str = "c1";

const PASSWORD: &str = "devpassword";

fn profile(id: &str) -> ConnProfile {
    ConnProfile {
        id: id.into(),
        name: id.into(),
        colour: "#3b82f6".into(),
        host: "127.0.0.1".into(),
        port: 3306,
        user: "root".into(),
        database: Some("poc".into()),
        // The container ships a self-signed cert, so strict verification would
        // (correctly) refuse it. See `refuses_a_self_signed_cert_by_default`.
        allow_invalid_certs: true,
    }
}

fn config() -> ConnProfile {
    profile(C)
}

/// Attach a tab to the shared test connection.
///
/// Tabs no longer auto-create: a tab needs a connection, and guessing which one
/// is exactly the ambient-state mistake Stage 2 exists to avoid. Every tab is
/// bound explicitly, here as in the app.
async fn attach(state: &AppState, tab: &str) {
    session::open_tab(state, C, tab).await.unwrap();
}

async fn connected() -> AppState {
    let state = AppState::default();
    session::connect(&state, config(), PASSWORD.into())
        .await
        .expect("connect failed — is the fixture container up?");
    // Tabs are bound to a connection now, so attach the shared test tab.
    session::open_tab(&state, C, T).await.unwrap();
    state
}

/// Number of connections this client currently holds on the server. Used to
/// prove tab connections are lazy and are actually released on close.
async fn server_conn_count(state: &AppState) -> i64 {
    let server = session::server(state, C).await.unwrap();
    let mut meta = server.meta.lock().await;
    sqlx::query(
        "SELECT COUNT(*) AS n FROM information_schema.processlist \
         WHERE user = ? AND id <> CONNECTION_ID()",
    )
    .bind("root")
    .fetch_one(&mut *meta)
    .await
    .unwrap()
    .try_get::<i64, _>("n")
    .unwrap()
}

fn rows_of(o: &Outcome) -> &Vec<Vec<CellValue>> {
    match o {
        Outcome::Rows { rows, .. } => rows,
        other => panic!("expected rows, got {other:?}"),
    }
}

fn as_json(v: &CellValue) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

// ------------------------------------------------------------------ M1

#[tokio::test]
#[ignore]
async fn m1_connects_and_lists_databases() {
    let state = AppState::default();
    let info = session::connect(&state, config(), PASSWORD.into())
        .await
        .unwrap();
    assert!(
        info.server_version.starts_with('8'),
        "got {}",
        info.server_version
    );
    assert!(info.databases.iter().any(|d| d == "poc"));

    // An unattached tab has no session at all.
    let st = session::tab_status(&state, T).await.unwrap();
    assert!(!st.connected, "an unattached tab reported a live session");

    // And attaching one must NOT open a server connection: they are lazy, so a
    // window full of idle tabs costs nothing until something actually runs.
    session::open_tab(&state, C, T).await.unwrap();
    let st = session::tab_status(&state, T).await.unwrap();
    assert!(st.connected);
    assert_eq!(st.connection_id, 0, "a tab connection was opened eagerly");

    session::disconnect(&state, C).await.unwrap();
}

#[tokio::test]
#[ignore]
async fn m1_wrong_password_is_readable_and_does_not_panic() {
    let state = AppState::default();
    let bad = config();
    let err = session::connect(&state, bad, "definitely-wrong".into())
        .await
        .unwrap_err();
    assert!(err.contains("Access denied"), "unfriendly error: {err}");
}

#[tokio::test]
#[ignore]
async fn refuses_a_self_signed_cert_by_default() {
    // The secure default must actually refuse. If this ever starts passing,
    // VerifyIdentity has silently stopped being enforced.
    let state = AppState::default();
    let mut strict = config();
    strict.allow_invalid_certs = false;
    let err = session::connect(&state, strict, PASSWORD.into())
        .await
        .unwrap_err();
    assert!(
        err.to_lowercase().contains("tls") || err.to_lowercase().contains("certificate"),
        "expected a TLS refusal, got: {err}"
    );
}

// ------------------------------------------------------------------ M2

#[tokio::test]
#[ignore]
async fn m2_null_is_json_null_and_distinct_from_the_string_null() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT c_literal_null FROM types_zoo ORDER BY id",
        true,
        None,
    )
    .await
    .unwrap();
    let rows = rows_of(&r.statements[0].outcome);
    assert_eq!(as_json(&rows[0][0]), serde_json::json!("NULL")); // the string
    assert_eq!(as_json(&rows[1][0]), serde_json::json!("null")); // the string
    assert_eq!(as_json(&rows[2][0]), serde_json::Value::Null); // a real NULL
}

#[tokio::test]
#[ignore]
async fn m2_bigint_beyond_2_53_survives_as_text() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT c_bigint, c_bigint_u FROM types_zoo WHERE id = 2",
        true,
        None,
    )
    .await
    .unwrap();
    let rows = rows_of(&r.statements[0].outcome);
    // 9007199254740993 is 2^53+1: as a JSON number it would round to ...992.
    assert_eq!(as_json(&rows[0][0]), serde_json::json!("9007199254740993"));
    assert_eq!(as_json(&rows[0][1]), serde_json::json!("9007199254740993"));
}

#[tokio::test]
#[ignore]
async fn m2_small_ints_stay_json_numbers() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT c_int FROM types_zoo WHERE id = 1",
        true,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!(2000000)
    );
}

#[tokio::test]
#[ignore]
async fn m2_decimal_keeps_full_precision_as_text() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT c_decimal FROM types_zoo WHERE id = 1",
        true,
        None,
    )
    .await
    .unwrap();
    // Through f64 this would not round-trip exactly.
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!("12345678901234.5678")
    );
}

#[tokio::test]
#[ignore]
async fn m2_every_column_type_decodes_without_panicking() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SELECT * FROM types_zoo", true, None)
        .await
        .unwrap();
    match &r.statements[0].outcome {
        Outcome::Rows { columns, rows, .. } => {
            assert_eq!(columns.len(), 26);
            assert_eq!(rows.len(), 3);
            // Row 3 is entirely NULL apart from the PK.
            assert!(rows[2][1..].iter().all(|c| matches!(c, CellValue::Null)));
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

// ----------------------------------------------------------------- M2b

#[tokio::test]
#[ignore]
async fn m2b_script_stops_at_the_first_error_and_keeps_prior_results() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT 1; SELECT * FROM no_such_table; SELECT 3",
        true,
        None,
    )
    .await
    .unwrap();
    // Statement 3 must be absent, and 1's rows must survive 2's failure.
    assert_eq!(r.statements.len(), 2);
    assert_eq!(r.aborted_at, Some(1));
    assert!(matches!(r.statements[0].outcome, Outcome::Rows { .. }));
    match &r.statements[1].outcome {
        Outcome::Error { message } => assert!(message.contains("no_such_table"), "{message}"),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn m2b_semicolon_in_a_string_does_not_split_the_script() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SELECT 'a;b' AS v", true, None)
        .await
        .unwrap();
    assert_eq!(r.statements.len(), 1);
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!("a;b")
    );
}

#[tokio::test]
#[ignore]
async fn m2b_show_and_describe_return_rows_not_affected_counts() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SHOW TABLES; DESCRIBE users", true, None)
        .await
        .unwrap();
    assert_eq!(r.statements[0].kind, StatementKind::RowReturning);
    assert!(!rows_of(&r.statements[0].outcome).is_empty());
    assert_eq!(rows_of(&r.statements[1].outcome).len(), 5); // users has 5 columns
                                                            // Never rewritten, even though they return rows.
    assert!(r.statements.iter().all(|s| s.effective_sql.is_none()));
}

// ------------------------------------------------------------------ M3

#[tokio::test]
#[ignore]
async fn m3_lists_tables_views_and_columns_and_caches_them() {
    let state = connected().await;
    let tables = schema::list_tables(&state, C, "poc").await.unwrap();
    assert!(tables
        .iter()
        .any(|t| t.name == "users" && t.kind == "BASE TABLE"));
    assert!(tables
        .iter()
        .any(|t| t.name == "user_totals" && t.kind == "VIEW"));

    let cols = schema::list_columns(&state, C, "poc", "users")
        .await
        .unwrap();
    assert_eq!(cols.len(), 5);
    let email = cols.iter().find(|c| c.name == "email").unwrap();
    assert!(!email.nullable);
    assert_eq!(email.key.as_deref(), Some("UNI"));

    // The cache's job is to keep information_schema round trips flat on repeat
    // reads. Assert on the counter rather than inferring from timing.
    let server = session::server(&state, C).await.unwrap();
    let before = server.introspection_count.load(Ordering::SeqCst);
    assert_eq!(
        schema::list_tables(&state, C, "poc").await.unwrap().len(),
        tables.len()
    );
    assert_eq!(
        schema::list_columns(&state, C, "poc", "users")
            .await
            .unwrap()
            .len(),
        5
    );
    assert_eq!(
        server.introspection_count.load(Ordering::SeqCst),
        before,
        "cached reads still hit information_schema"
    );
}

// ------------------------------------------------------------------ M5

#[tokio::test]
#[ignore]
async fn m5_auto_limit_truncates_a_big_table_and_reports_the_rewrite() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SELECT * FROM big", true, None)
        .await
        .unwrap();
    let s = &r.statements[0];
    assert_eq!(
        s.effective_sql.as_deref(),
        Some("SELECT * FROM big LIMIT 5000")
    );
    match &s.outcome {
        Outcome::Rows {
            rows, truncated, ..
        } => {
            assert_eq!(rows.len(), 5000);
            assert!(*truncated, "truncation chip would not appear");
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn m5_disabling_auto_limit_returns_the_whole_table() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SELECT * FROM big", false, None)
        .await
        .unwrap();
    assert!(r.statements[0].effective_sql.is_none());
    assert_eq!(rows_of(&r.statements[0].outcome).len(), 7500);
}

#[tokio::test]
#[ignore]
async fn m5_cancel_kills_a_slow_query_promptly() {
    use std::time::{Duration, Instant};
    let state = std::sync::Arc::new(connected().await);

    let killer = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        session::cancel_query(&killer, T)
            .await
            .expect("cancel failed");
    });

    let started = Instant::now();
    let r = exec::run_script(&state, T, "SELECT SLEEP(30)", true, None)
        .await
        .unwrap();
    let elapsed = started.elapsed();

    // The whole point: control returns in about the time to KILL, not 30s.
    assert!(
        elapsed < Duration::from_secs(5),
        "took {elapsed:?} — cancel did not work"
    );
    // A KILLed SLEEP() returns 1 rather than erroring, so the outcome is a
    // perfectly ordinary row. Only the `cancelled` flag distinguishes it from
    // a query that genuinely finished — without it the UI would show nothing.
    assert!(r.cancelled, "script was not reported as cancelled: {r:?}");
}

#[tokio::test]
#[ignore]
async fn m5_cancel_abandons_the_rest_of_the_script() {
    use std::time::Duration;
    let state = std::sync::Arc::new(connected().await);
    let killer = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        session::cancel_query(&killer, T).await.ok();
    });
    let r = exec::run_script(&state, T, "SELECT SLEEP(30); SELECT 999", true, None)
        .await
        .unwrap();
    // Statement 2 must never run — matches the stop-at-first-error rule.
    assert_eq!(
        r.statements.len(),
        1,
        "the remainder of the script was not abandoned"
    );
}

// --------------------------------------------------------------- USE / db

#[tokio::test]
#[ignore]
async fn a_bare_use_in_a_script_updates_the_active_database() {
    let state = connected().await;
    exec::run_script(&state, T, "USE mysql; SELECT 1", true, None)
        .await
        .unwrap();
    let tab = session::tab_if_open(&state, T).await.unwrap();
    assert_eq!(tab.current_db.lock().await.as_deref(), Some("mysql"));
}

// --------------------------------------------------------------- Phase 6

#[tokio::test]
#[ignore]
async fn timeout_kills_a_slow_query_and_is_reported_separately_from_cancel() {
    use std::time::{Duration, Instant};
    let state = connected().await;

    let started = Instant::now();
    let r = exec::run_script(&state, T, "SELECT SLEEP(30)", true, Some(1))
        .await
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(6),
        "timeout did not fire: {elapsed:?}"
    );
    assert!(r.timed_out, "timed_out flag not set: {r:?}");
}

#[tokio::test]
#[ignore]
async fn timeout_of_zero_or_none_means_no_limit() {
    let state = connected().await;
    // A quick query must not be affected by either spelling of "off".
    let r = exec::run_script(&state, T, "SELECT SLEEP(0.2)", true, None)
        .await
        .unwrap();
    assert!(!r.timed_out);
    let r = exec::run_script(&state, T, "SELECT SLEEP(0.2)", true, Some(0))
        .await
        .unwrap();
    assert!(!r.timed_out);
}

#[tokio::test]
#[ignore]
async fn the_connection_stays_usable_after_a_timeout() {
    // The real risk of a naive timeout: dropping the future mid-protocol
    // poisons the connection and every later query fails.
    let state = connected().await;
    let timed = exec::run_script(&state, T, "SELECT SLEEP(30)", true, Some(1))
        .await
        .unwrap();
    assert!(timed.timed_out);

    let after = exec::run_script(&state, T, "SELECT 1 + 1 AS two", true, None)
        .await
        .unwrap();
    assert_eq!(
        as_json(&rows_of(&after.statements[0].outcome)[0][0]),
        serde_json::json!(2),
        "connection was left unusable after the timeout"
    );
}

// ------------------------------------------------------- Phase 5b (linting)

#[tokio::test]
#[ignore]
async fn lint_uses_the_live_schema_cache() {
    let state = connected().await;
    session::use_database(&state, T, "poc").await.unwrap();
    // Warm the cache the way the sidebar would.
    schema::list_columns(&state, C, "poc", "users")
        .await
        .unwrap();

    let cached = schema::lint_schema(&session::server(&state, C).await.unwrap(), Some("poc")).await;

    let good = db_query_lib::lint::lint("SELECT email FROM users", &cached);
    assert!(good.is_empty(), "real column flagged: {good:?}");

    let bad = db_query_lib::lint::lint("SELECT emial FROM users", &cached);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].message.contains("no column `emial`"));
}

// ================================================================ Stage 1 / Phase 1
// Per-tab sessions. These are the tests that justify the architecture.

/// **N5.** The whole point of connection-per-tab: a slow query in one tab must
/// not stall another. If this regresses, tabs are cosmetic.
#[tokio::test]
#[ignore]
async fn n5_a_slow_query_in_one_tab_does_not_block_another() {
    use std::time::{Duration, Instant};
    let state = std::sync::Arc::new(connected().await);
    attach(&state, "slow-tab").await;
    attach(&state, "quick-tab").await;

    let slow = state.clone();
    let handle = tokio::spawn(async move {
        exec::run_script(&slow, "slow-tab", "SELECT SLEEP(5)", true, None).await
    });

    // Give the slow tab a moment to actually take its connection.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started = Instant::now();
    let quick = exec::run_script(&state, "quick-tab", "SELECT 1 AS one", true, None)
        .await
        .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(
        as_json(&rows_of(&quick.statements[0].outcome)[0][0]),
        serde_json::json!(1)
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "second tab waited {elapsed:?} — it is serialized behind the first"
    );

    handle.await.unwrap().unwrap();
}

/// Cancelling one tab must leave every other tab alone.
#[tokio::test]
#[ignore]
async fn cancel_targets_only_the_requesting_tab() {
    use std::time::Duration;
    let state = std::sync::Arc::new(connected().await);
    attach(&state, "tab-a").await;
    attach(&state, "tab-b").await;

    let victim = state.clone();
    let survivor = state.clone();

    let a = tokio::spawn(async move {
        exec::run_script(&victim, "tab-a", "SELECT SLEEP(30)", true, None).await
    });
    let b = tokio::spawn(async move {
        exec::run_script(&survivor, "tab-b", "SELECT SLEEP(3)", true, None).await
    });

    tokio::time::sleep(Duration::from_millis(600)).await;
    session::cancel_query(&state, "tab-a").await.unwrap();

    let ra = a.await.unwrap().unwrap();
    let rb = b.await.unwrap().unwrap();

    assert!(ra.cancelled, "tab-a was not cancelled");
    assert!(!rb.cancelled, "tab-b was cancelled by tab-a's request");
    assert!(matches!(rb.statements[0].outcome, Outcome::Rows { .. }));
}

/// Connections are lazy: idle tabs cost nothing, and closing a tab gives its
/// connection back rather than leaking it until disconnect.
#[tokio::test]
#[ignore]
async fn tab_connections_are_lazy_and_released_on_close() {
    let state = connected().await;
    let baseline = server_conn_count(&state).await; // killer only

    // Registering a tab must not open anything.
    attach(&state, "lazy").await;
    assert_eq!(
        server_conn_count(&state).await,
        baseline,
        "open_tab opened a connection eagerly"
    );

    // First run opens exactly one.
    exec::run_script(&state, "lazy", "SELECT 1", true, None)
        .await
        .unwrap();
    assert_eq!(server_conn_count(&state).await, baseline + 1);

    // A second tab opens exactly one more.
    attach(&state, "lazy2").await;
    exec::run_script(&state, "lazy2", "SELECT 1", true, None)
        .await
        .unwrap();
    assert_eq!(server_conn_count(&state).await, baseline + 2);

    session::close_tab(&state, "lazy").await.unwrap();
    session::close_tab(&state, "lazy2").await.unwrap();
    assert_eq!(
        server_conn_count(&state).await,
        baseline,
        "closing tabs did not release their connections"
    );
}

/// Each tab keeps its own `USE`. Surprising to some users, but it is what makes
/// a script behave the same way it would run standalone.
#[tokio::test]
#[ignore]
async fn each_tab_has_its_own_active_database() {
    let state = connected().await;
    attach(&state, "tab-a").await;
    attach(&state, "tab-b").await;
    session::use_database(&state, "tab-a", "mysql")
        .await
        .unwrap();
    session::use_database(&state, "tab-b", "poc").await.unwrap();

    let a = session::tab_status(&state, "tab-a").await.unwrap();
    let b = session::tab_status(&state, "tab-b").await.unwrap();
    assert_eq!(a.current_database.as_deref(), Some("mysql"));
    assert_eq!(b.current_database.as_deref(), Some("poc"));

    // And the server agrees, not just our bookkeeping.
    let r = exec::run_script(&state, "tab-a", "SELECT DATABASE() AS d", true, None)
        .await
        .unwrap();
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!("mysql")
    );
    let r = exec::run_script(&state, "tab-b", "SELECT DATABASE() AS d", true, None)
        .await
        .unwrap();
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!("poc")
    );
}

/// A `USE` inside one tab's script must not move any other tab.
#[tokio::test]
#[ignore]
async fn a_use_in_one_tabs_script_does_not_move_another_tab() {
    let state = connected().await;
    attach(&state, "tab-a").await;
    attach(&state, "tab-b").await;
    session::use_database(&state, "tab-a", "poc").await.unwrap();
    session::use_database(&state, "tab-b", "poc").await.unwrap();

    exec::run_script(&state, "tab-a", "USE mysql; SELECT 1", true, None)
        .await
        .unwrap();

    let a = session::tab_status(&state, "tab-a").await.unwrap();
    let b = session::tab_status(&state, "tab-b").await.unwrap();
    assert_eq!(a.current_database.as_deref(), Some("mysql"));
    assert_eq!(b.current_database.as_deref(), Some("poc"), "tab-b drifted");
}

/// MySQL's `wait_timeout` reaps idle connections (8 hours by default), so a tab
/// left open overnight finds its connection gone. Simulated here by killing the
/// connection outright: the next run must reconnect transparently, land back in
/// the right database, and not surface an error.
#[tokio::test]
#[ignore]
async fn a_reaped_tab_connection_is_reopened_transparently() {
    let state = connected().await;
    attach(&state, "reaped").await;
    session::use_database(&state, "reaped", "poc")
        .await
        .unwrap();
    exec::run_script(&state, "reaped", "SELECT 1", true, None)
        .await
        .unwrap();

    let old_id = session::tab_status(&state, "reaped")
        .await
        .unwrap()
        .connection_id;
    assert!(old_id > 0);

    // KILL (not KILL QUERY) drops the whole connection, exactly as wait_timeout would.
    {
        let server = session::server(&state, C).await.unwrap();
        let mut meta = server.meta.lock().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("KILL {old_id}")))
            .execute(&mut *meta)
            .await
            .ok();
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let r = exec::run_script(&state, "reaped", "SELECT DATABASE() AS d", true, None)
        .await
        .expect("reconnect failed — the tab is permanently broken");

    let new_id = session::tab_status(&state, "reaped")
        .await
        .unwrap()
        .connection_id;
    assert_ne!(new_id, old_id, "connection was not actually replaced");
    // The replacement session must land back in the tab's database.
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!("poc"),
        "reconnected session lost the tab's active database"
    );
}

/// disconnect must take every tab connection down with it.
#[tokio::test]
#[ignore]
async fn disconnect_releases_every_tab_connection() {
    let state = connected().await;
    attach(&state, "d1").await;
    attach(&state, "d2").await;
    let baseline = server_conn_count(&state).await;
    exec::run_script(&state, "d1", "SELECT 1", true, None)
        .await
        .unwrap();
    exec::run_script(&state, "d2", "SELECT 1", true, None)
        .await
        .unwrap();
    assert_eq!(server_conn_count(&state).await, baseline + 2);

    session::disconnect(&state, C).await.unwrap();

    // Reconnect on a fresh state to count what the old one left behind.
    let probe = connected().await;
    assert_eq!(
        server_conn_count(&probe).await,
        baseline,
        "tab connections outlived disconnect"
    );
    session::disconnect(&probe, C).await.ok();
}

// ================================================================ Stage 1 / Phase 7

/// `MAX_ROWS` caps one statement; this caps the whole script. Without it a
/// ten-SELECT script holds 50,000 rows, and that multiplies by open tabs.
#[tokio::test]
#[ignore]
async fn a_long_script_is_bounded_by_the_script_wide_row_ceiling() {
    let state = connected().await;
    // Five statements × 5000 auto-limited rows = 25,000, over the 20,000 budget.
    let sql = "SELECT * FROM big; SELECT * FROM big; SELECT * FROM big; \
               SELECT * FROM big; SELECT * FROM big";
    let r = exec::run_script(&state, T, sql, true, None).await.unwrap();

    let total: usize = r
        .statements
        .iter()
        .map(|s| match &s.outcome {
            Outcome::Rows { rows, .. } => rows.len(),
            _ => 0,
        })
        .sum();

    assert_eq!(r.statements.len(), 5, "every statement should still run");
    assert!(
        total <= exec::MAX_SCRIPT_ROWS,
        "script held {total} rows, over the {} ceiling",
        exec::MAX_SCRIPT_ROWS
    );
    // The statement where the budget ran out must say so, not silently shrink.
    let last = r.statements.last().unwrap();
    match &last.outcome {
        Outcome::Rows { truncated, .. } => {
            assert!(
                *truncated,
                "budget exhaustion was not reported as truncation"
            )
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

/// A script that fits under the ceiling must be entirely unaffected by it.
#[tokio::test]
#[ignore]
async fn a_short_script_is_untouched_by_the_ceiling() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SELECT * FROM big; SELECT 1", true, None)
        .await
        .unwrap();
    match &r.statements[0].outcome {
        Outcome::Rows { rows, .. } => assert_eq!(rows.len(), 5000),
        other => panic!("expected rows, got {other:?}"),
    }
    match &r.statements[1].outcome {
        Outcome::Rows {
            rows, truncated, ..
        } => {
            assert_eq!(rows.len(), 1);
            assert!(!truncated, "false truncation on a small result");
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

// ================================================================ Stage 2 / Phase 1
// Several live connections at once. These justify the connection registry.

const C2: &str = "c2";

/// Open a second connection to the same server, with its own tab.
async fn connect_second(state: &AppState, tab: &str) {
    session::connect(state, profile(C2), PASSWORD.into())
        .await
        .unwrap();
    session::open_tab(state, C2, tab).await.unwrap();
}

/// **C4.** Two connections, two tabs, two independent sessions. `SELECT
/// DATABASE()` asks the server, so this is not just our own bookkeeping.
#[tokio::test]
#[ignore]
async fn two_connections_keep_separate_sessions() {
    let state = connected().await;
    connect_second(&state, "c2-tab").await;

    session::use_database(&state, T, "poc").await.unwrap();
    session::use_database(&state, "c2-tab", "mysql")
        .await
        .unwrap();

    let a = exec::run_script(&state, T, "SELECT DATABASE() AS d", true, None)
        .await
        .unwrap();
    let b = exec::run_script(&state, "c2-tab", "SELECT DATABASE() AS d", true, None)
        .await
        .unwrap();

    assert_eq!(
        as_json(&rows_of(&a.statements[0].outcome)[0][0]),
        serde_json::json!("poc")
    );
    assert_eq!(
        as_json(&rows_of(&b.statements[0].outcome)[0][0]),
        serde_json::json!("mysql")
    );
}

/// **C5.** A slow query on one connection must not delay another. Stage 1
/// proved this across tabs; this proves it across servers.
#[tokio::test]
#[ignore]
async fn a_slow_query_on_one_connection_does_not_block_another() {
    use std::time::{Duration, Instant};
    let state = std::sync::Arc::new(connected().await);
    connect_second(&state, "c2-tab").await;

    let slow = state.clone();
    let handle =
        tokio::spawn(
            async move { exec::run_script(&slow, T, "SELECT SLEEP(5)", true, None).await },
        );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started = Instant::now();
    exec::run_script(&state, "c2-tab", "SELECT 1", true, None)
        .await
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "second connection waited {elapsed:?} — connections are serialized"
    );
    handle.await.unwrap().unwrap();
}

/// Disconnecting one connection must leave the other completely untouched —
/// its tabs, its sessions, its schema cache.
#[tokio::test]
#[ignore]
async fn disconnecting_one_connection_leaves_the_other_working() {
    let state = connected().await;
    connect_second(&state, "c2-tab").await;

    // Warm both so there is real state to lose.
    exec::run_script(&state, T, "SELECT 1", true, None)
        .await
        .unwrap();
    exec::run_script(&state, "c2-tab", "SELECT 1", true, None)
        .await
        .unwrap();
    schema::list_tables(&state, C2, "poc").await.unwrap();

    session::disconnect(&state, C).await.unwrap();

    // The closed connection is gone, and so are its tabs.
    assert!(session::server(&state, C).await.is_err());
    assert!(
        exec::run_script(&state, T, "SELECT 1", true, None)
            .await
            .is_err(),
        "a tab of a disconnected connection still ran"
    );

    // The survivor is entirely unaffected.
    let r = exec::run_script(&state, "c2-tab", "SELECT 2 AS two", true, None)
        .await
        .unwrap();
    assert_eq!(
        as_json(&rows_of(&r.statements[0].outcome)[0][0]),
        serde_json::json!(2)
    );
    assert!(!schema::list_tables(&state, C2, "poc")
        .await
        .unwrap()
        .is_empty());
}

/// Each connection has its own schema cache, so warming one must not make the
/// other look warm — that would silently disable the other's lint checks.
#[tokio::test]
#[ignore]
async fn schema_caches_are_per_connection() {
    let state = connected().await;
    connect_second(&state, "c2-tab").await;

    schema::list_columns(&state, C, "poc", "users")
        .await
        .unwrap();

    let a = session::server(&state, C).await.unwrap();
    let b = session::server(&state, C2).await.unwrap();
    assert!(!schema::lint_schema(&a, Some("poc")).await.is_empty());
    assert!(
        schema::lint_schema(&b, Some("poc")).await.is_empty(),
        "warming one connection's cache warmed another's"
    );
}

/// `list_connections` is what the rail will render, so its counts must be real.
#[tokio::test]
#[ignore]
async fn list_connections_reports_live_state_and_tab_counts() {
    let state = connected().await;
    connect_second(&state, "c2-tab").await;
    session::open_tab(&state, C, "extra").await.unwrap();

    let mut list = session::list_connections(&state).await;
    list.sort_by(|x, y| x.id.cmp(&y.id));
    assert_eq!(list.len(), 2);

    assert_eq!(list[0].id, C);
    assert!(list[0].connected);
    assert!(list[0]
        .server_version
        .as_deref()
        .unwrap_or("")
        .starts_with('8'));
    assert_eq!(list[0].open_tabs, 2, "T and extra");
    assert_eq!(list[0].running_tabs, 0);
    assert_eq!(list[1].id, C2);
    assert_eq!(list[1].open_tabs, 1);

    session::disconnect(&state, C).await.unwrap();
    let list = session::list_connections(&state).await;
    assert_eq!(
        list.len(),
        1,
        "a disconnected connection is still listed as live"
    );
    assert_eq!(list[0].id, C2);
}

/// Cancelling a tab must not reach across connections.
#[tokio::test]
#[ignore]
async fn cancel_does_not_reach_across_connections() {
    use std::time::Duration;
    let state = std::sync::Arc::new(connected().await);
    connect_second(&state, "c2-tab").await;

    let a = state.clone();
    let b = state.clone();
    let ha =
        tokio::spawn(async move { exec::run_script(&a, T, "SELECT SLEEP(30)", true, None).await });
    let hb =
        tokio::spawn(
            async move { exec::run_script(&b, "c2-tab", "SELECT SLEEP(3)", true, None).await },
        );

    tokio::time::sleep(Duration::from_millis(600)).await;
    session::cancel_query(&state, T).await.unwrap();

    let ra = ha.await.unwrap().unwrap();
    let rb = hb.await.unwrap().unwrap();
    assert!(ra.cancelled, "the targeted tab was not cancelled");
    assert!(
        !rb.cancelled,
        "a tab on another connection was cancelled too"
    );
}

// ================================================================ Stage 2 / Phase 2
// Secret-leakage audit. The risk that matters most is a password ending up
// somewhere ordinary — an error string, a log line, a Debug print — because it
// is silent and permanent once it reaches a backup.

/// A password distinctive enough that finding it in any output is unambiguous.
const CANARY: &str = "zzCANARYzz-9f3a-do-not-leak";

#[tokio::test]
#[ignore]
async fn a_failed_connection_never_echoes_the_password() {
    let state = AppState::default();
    let err = session::connect(&state, profile("leak-test"), CANARY.into())
        .await
        .unwrap_err();
    assert!(
        !err.contains(CANARY),
        "the password appeared in a connection error: {err}"
    );
    // And it is still a useful message, not just a redacted one.
    assert!(err.contains("Access denied"), "unhelpful error: {err}");
}

#[tokio::test]
#[ignore]
async fn a_failed_connection_to_a_dead_host_never_echoes_the_password() {
    let state = AppState::default();
    let mut p = profile("leak-host");
    p.host = "127.0.0.1".into();
    p.port = 1; // nothing listens here
    let err = session::connect(&state, p, CANARY.into())
        .await
        .unwrap_err();
    assert!(
        !err.contains(CANARY),
        "the password appeared in a connect/IO error: {err}"
    );
}

#[tokio::test]
#[ignore]
async fn a_tls_refusal_never_echoes_the_password() {
    // Strict verification against the container's self-signed cert fails; the
    // resulting TLS error must not carry the credential either.
    let state = AppState::default();
    let mut p = profile("leak-tls");
    p.allow_invalid_certs = false;
    let err = session::connect(&state, p, CANARY.into())
        .await
        .unwrap_err();
    assert!(
        !err.contains(CANARY),
        "the password appeared in a TLS error: {err}"
    );
}

#[tokio::test]
#[ignore]
async fn a_query_error_never_echoes_the_password() {
    let state = connected().await;
    let r = exec::run_script(&state, T, "SELECT * FROM no_such_table", true, None)
        .await
        .unwrap();
    let msg = format!("{:?}", r.statements[0].outcome);
    assert!(
        !msg.contains(PASSWORD),
        "the password leaked into a query error: {msg}"
    );
}

/// The profile is the shape that gets written to disk, so its serialised form
/// is the last line of defence for the config file.
#[tokio::test]
#[ignore]
async fn a_serialised_profile_carries_no_secret() {
    let json = serde_json::to_string(&profile("ser")).unwrap();
    assert!(!json.to_lowercase().contains("password"), "{json}");
    assert!(!json.contains(PASSWORD), "{json}");
    assert!(!json.contains(CANARY), "{json}");
}
