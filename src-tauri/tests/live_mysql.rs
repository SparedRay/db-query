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
        kind: Default::default(),
        url: String::new(),
        auth: Default::default(),
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
    let mut meta = server.mysql_meta().await.expect("a MySQL connection");
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

/// Found by the first build ever pointed at a real database.
///
/// A type name does not tell you what a column holds. MySQL reports a string
/// column with a **binary collation** under the same names as a BLOB, so
/// readable words came back as `<binary, 16 bytes>`; and sqlx's typed
/// accessors check type *compatibility* before decoding anything, so JSON and
/// BIT came back as `<undecodable>`.
#[tokio::test]
#[ignore]
async fn awkward_column_types_decode_as_what_they_actually_hold() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT v_utf8mb4, v_latin1, v_binary_coll, c_char, t_text, e_enum, s_set, \
         j_json, b_bit FROM awkward_types WHERE id = 1",
        true,
        None,
    )
    .await
    .unwrap();
    let row = &rows_of(&r.statements[0].outcome)[0];

    // Text stays text, whatever charset it is stored in.
    assert_eq!(as_json(&row[0]), serde_json::json!("héllo wörld"));
    assert_eq!(as_json(&row[1]), serde_json::json!("héllo latin1"));
    // The regression: utf8mb4_bin is reported as VARBINARY and is still text.
    assert_eq!(as_json(&row[2]), serde_json::json!("héllo bincoll"));
    assert_eq!(as_json(&row[3]), serde_json::json!("ábc"));
    assert_eq!(as_json(&row[4]), serde_json::json!("latin1 text é"));
    assert_eq!(as_json(&row[5]), serde_json::json!("alpha"));
    assert_eq!(as_json(&row[6]), serde_json::json!("x,y"));
    // Was "<undecodable>".
    assert_eq!(as_json(&row[7]), serde_json::json!("{\"k\": \"v\"}"));
    // BIT is an unsigned integer of N bits: b'10101010' is 170.
    assert_eq!(as_json(&row[8]), serde_json::json!(170));
}

/// The other half of the same fix: genuinely binary columns must **stay**
/// binary. A fix that turned every blob into mojibake would pass the test
/// above and be worse than the bug.
///
/// Stage 12 changed how that is *said* on the wire — from the placeholder
/// string `"<binary, 3 bytes>"` to `{"bytes": 3}` — because a string could not
/// be told apart from a real value of those characters, which is what forced
/// the SQL export to guess from the column type. The fact being asserted is
/// unchanged: these two columns are binary and the length is known.
#[tokio::test]
#[ignore]
async fn genuinely_binary_columns_are_still_reported_as_binary() {
    let state = connected().await;
    let r = exec::run_script(
        &state,
        T,
        "SELECT vb_varbinary, bl_blob FROM awkward_types WHERE id = 1",
        true,
        None,
    )
    .await
    .unwrap();
    let row = &rows_of(&r.statements[0].outcome)[0];
    assert_eq!(as_json(&row[0]), serde_json::json!({ "bytes": 3 }));
    assert_eq!(as_json(&row[1]), serde_json::json!({ "bytes": 4 }));

    // And the whole point of the new shape: the SQL-INSERT export refuses these
    // on the strength of the *value*, with no reference to the column type.
    for cell in row.iter().take(2) {
        assert!(
            db_query_lib::sqlgen::literal(cell, db_query_lib::decode::TypeHint::Text).is_err(),
            "a binary value must be refused whatever its column claims to be"
        );
    }
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

    let good = db_query_lib::lint::lint(
        "SELECT email FROM users",
        &cached,
        db_query_lib::lint::Dialect::mysql(),
    );
    assert!(good.is_empty(), "real column flagged: {good:?}");

    let bad = db_query_lib::lint::lint(
        "SELECT emial FROM users",
        &cached,
        db_query_lib::lint::Dialect::mysql(),
    );
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
        let mut meta = server.mysql_meta().await.expect("a MySQL connection");
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

// ------------------------------------------------------- Stage 3: routines

/// E3. Procedures and functions are listed with the detail the tree needs to
/// show them separately and generate a call.
#[tokio::test]
#[ignore]
async fn routines_are_listed_with_parameters_and_return_types() {
    let state = connected().await;
    let routines = schema::list_routines(&state, C, "poc").await.unwrap();

    let names: Vec<&str> = routines.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"top_spenders"), "{names:?}");
    assert!(names.contains(&"order_count"), "{names:?}");
    assert!(names.contains(&"ping_poc"), "{names:?}");

    let proc = routines.iter().find(|r| r.name == "top_spenders").unwrap();
    assert_eq!(proc.kind, schema::RoutineKind::Procedure);
    assert_eq!(proc.returns, None, "a procedure returns nothing");
    assert_eq!(
        proc.params
            .iter()
            .map(|p| (p.name.as_str(), p.mode.as_str()))
            .collect::<Vec<_>>(),
        vec![("min_total", "IN"), ("max_rows", "IN")],
        "parameters must arrive in declaration order",
    );
    assert!(proc.params[0].data_type.starts_with("decimal"), "{proc:?}");

    let func = routines.iter().find(|r| r.name == "order_count").unwrap();
    assert_eq!(func.kind, schema::RoutineKind::Function);
    assert_eq!(func.returns.as_deref(), Some("int"));
    // The regression that motivated the ordinal_position filter: a function's
    // return value is row 0 in information_schema.parameters, with a NULL name
    // and NULL mode. Including it puts a nameless argument in every call.
    assert_eq!(
        func.params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["uid"],
        "row 0 is the RETURN type, not a parameter: {func:?}",
    );

    let noargs = routines.iter().find(|r| r.name == "ping_poc").unwrap();
    assert!(noargs.params.is_empty(), "{noargs:?}");
}

/// **E4.** The milestone check: examine a procedure, run the script it produced,
/// and read the definition back. If `DROP … IF EXISTS` were missing the rerun
/// fails; if the `DELIMITER` wrapper were missing the body is truncated at its
/// first internal semicolon. Both failures show up here as a changed body.
#[tokio::test]
#[ignore]
async fn examining_a_routine_produces_a_script_that_recreates_it_exactly() {
    let state = connected().await;

    let before = body_of(&state, "top_spenders").await;
    let script = schema::routine_ddl(
        &state,
        C,
        "poc",
        "top_spenders",
        schema::RoutineKind::Procedure,
    )
    .await
    .unwrap();

    assert!(script.contains("DROP PROCEDURE IF EXISTS"), "{script}");
    assert!(script.contains("DELIMITER $$"), "{script}");
    assert!(script.contains("USE `poc`;"), "{script}");

    // Run it exactly as the user would: one script through the real executor.
    let out = exec::run_script(&state, T, &script, false, None)
        .await
        .unwrap();
    for st in &out.statements {
        assert!(
            !matches!(st.outcome, Outcome::Error { .. }),
            "generated script failed: {:?}\n--- script ---\n{script}",
            st.outcome
        );
    }

    let after = body_of(&state, "top_spenders").await;
    assert_eq!(
        before, after,
        "the routine did not survive its own re-creation script",
    );
    // And the body really does contain the semicolons that make DELIMITER
    // necessary — otherwise this test would pass on a trivial routine.
    assert!(
        before.contains(';'),
        "fixture too simple to prove the point"
    );
}

/// Reads the stored body straight from the server, bypassing our generator, so
/// the comparison above cannot be fooled by a bug shared with it.
async fn body_of(state: &AppState, name: &str) -> String {
    let server = session::server(state, C).await.unwrap();
    let mut meta = server.mysql_meta().await.expect("a MySQL connection");
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SHOW CREATE PROCEDURE `poc`.`{name}`"
    )))
    .fetch_one(&mut *meta)
    .await
    .unwrap();
    row.try_get::<Option<String>, _>("Create Procedure")
        .unwrap()
        .expect("no privilege to read the routine body")
}

/// A function's DDL uses a different result column ("Create Function"), which is
/// the kind of detail that only fails against a real server.
#[tokio::test]
#[ignore]
async fn a_function_ddl_uses_its_own_result_column() {
    let state = connected().await;
    let script = schema::routine_ddl(
        &state,
        C,
        "poc",
        "order_count",
        schema::RoutineKind::Function,
    )
    .await
    .unwrap();
    assert!(
        script.contains("DROP FUNCTION IF EXISTS `order_count`;"),
        "{script}"
    );
    assert!(
        script.contains("CREATE") && script.contains("order_count"),
        "{script}"
    );
}

/// Asking for a routine that is not there must be a readable error, not a panic
/// and not an empty tab.
#[tokio::test]
#[ignore]
async fn a_missing_routine_is_a_readable_error() {
    let state = connected().await;
    let err = schema::routine_ddl(
        &state,
        C,
        "poc",
        "no_such_routine",
        schema::RoutineKind::Procedure,
    )
    .await
    .expect_err("should not succeed");
    assert!(!err.is_empty());
    assert!(!err.to_lowercase().contains("panic"), "{err}");
}

/// Routines are cached like tables, and `refresh` must clear both — a stale
/// routine list after a refresh is the same silent-wrong-answer class of bug as
/// the empty schema tree in Stage 0.
#[tokio::test]
#[ignore]
async fn refresh_clears_the_routine_cache_too() {
    let state = connected().await;
    let server = session::server(&state, C).await.unwrap();

    schema::list_routines(&state, C, "poc").await.unwrap();
    let before = server.introspection_count.load(Ordering::SeqCst);
    schema::list_routines(&state, C, "poc").await.unwrap();
    assert_eq!(
        server.introspection_count.load(Ordering::SeqCst),
        before,
        "the second call should have been served from cache",
    );

    schema::refresh(&state, C, "poc").await.unwrap();
    schema::list_routines(&state, C, "poc").await.unwrap();
    assert!(
        server.introspection_count.load(Ordering::SeqCst) > before,
        "refresh did not invalidate the routine cache",
    );
}

// ------------------------------------------ Stage 3: literals, for real

/// The escaper's tests assert what we *believe* MySQL accepts. This asserts
/// what it actually does: build a literal for each hostile string, send it
/// through a real INSERT, read it back, and require it to be unchanged.
///
/// This is the check that would catch an escaping rule that is subtly wrong —
/// the class of bug that silently corrupts an exported script.
#[tokio::test]
#[ignore]
async fn generated_string_literals_round_trip_through_the_server() {
    let state = connected().await;
    let hostile = vec![
        "plain",
        "it's",
        r"back\slash",
        r"both ' and \",
        "'; DROP TABLE users; --",
        "line\nbreak",
        "carriage\rreturn",
        "tab\there",
        "ctrl\u{1a}z",
        "nul\0byte",
        "héllo ★ 表 🔐",
        "double \"quotes\"",
        "percent % and underscore _",
        "trailing space ",
        "",
    ];

    exec::run_script(
        &state,
        T,
        "DROP TEMPORARY TABLE IF EXISTS lit_probe; \
         CREATE TEMPORARY TABLE lit_probe (i INT, v VARBINARY(255));",
        false,
        None,
    )
    .await
    .unwrap();

    for (i, s) in hostile.iter().enumerate() {
        let sql = format!(
            "INSERT INTO lit_probe (i, v) VALUES ({i}, {});",
            db_query_lib::sqlgen::quote_string(s)
        );
        let out = exec::run_script(&state, T, &sql, false, None)
            .await
            .unwrap();
        for st in &out.statements {
            assert!(
                !matches!(st.outcome, Outcome::Error { .. }),
                "literal for {s:?} was rejected: {:?}\n{sql}",
                st.outcome
            );
        }
    }

    // Read back as hex so the comparison cannot be blurred by the decoder's own
    // text handling — this is about the bytes the server stored.
    let out = exec::run_script(
        &state,
        T,
        "SELECT i, HEX(v) AS h FROM lit_probe ORDER BY i;",
        false,
        None,
    )
    .await
    .unwrap();
    let Outcome::Rows { rows, .. } = &out.statements[0].outcome else {
        panic!("expected rows: {:?}", out.statements[0].outcome);
    };
    assert_eq!(rows.len(), hostile.len());

    for (row, expected) in rows.iter().zip(&hostile) {
        let CellValue::Text(hex) = &row[1] else {
            panic!("expected hex text, got {:?}", row[1])
        };
        let got = hex::decode_lossy(hex);
        assert_eq!(
            got, *expected,
            "round trip changed the value: {expected:?} came back as {got:?}",
        );
    }
}

/// Tiny hex decoder, so the assertion above compares bytes rather than trusting
/// the same decode path the rest of the app uses.
mod hex {
    pub fn decode_lossy(h: &str) -> String {
        let bytes: Vec<u8> = h
            .as_bytes()
            .chunks(2)
            .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A generated SELECT must actually run, and return no more than it asked for.
#[tokio::test]
#[ignore]
async fn a_generated_select_runs_and_honours_its_limit() {
    let state = connected().await;
    let sql = db_query_lib::sqlgen::generate_select("poc", "big", 7).unwrap();
    let out = exec::run_script(&state, T, &sql, false, None)
        .await
        .unwrap();
    let Outcome::Rows { rows, .. } = &out.statements[0].outcome else {
        panic!("expected rows: {:?}", out.statements[0].outcome);
    };
    assert_eq!(rows.len(), 7, "the generated LIMIT was not honoured");
}

/// Auto-LIMIT must leave a generated SELECT alone: it already carries an
/// explicit LIMIT, and appending a second one is a syntax error.
#[tokio::test]
#[ignore]
async fn auto_limit_does_not_touch_a_generated_select() {
    let state = connected().await;
    let sql = db_query_lib::sqlgen::generate_select("poc", "big", 3).unwrap();
    let out = exec::run_script(&state, T, &sql, true, None).await.unwrap();
    assert!(
        out.statements[0].effective_sql.is_none(),
        "auto-LIMIT rewrote a statement that already had one: {:?}",
        out.statements[0].effective_sql
    );
    let Outcome::Rows { rows, .. } = &out.statements[0].outcome else {
        panic!("expected rows")
    };
    assert_eq!(rows.len(), 3);
}

/// The generated CALL is deliberately not runnable until its placeholders are
/// filled in. Proving that means proving it fails — which is the design.
#[tokio::test]
#[ignore]
async fn a_generated_call_becomes_runnable_once_filled_in() {
    let state = connected().await;
    let routines = schema::list_routines(&state, C, "poc").await.unwrap();
    let proc = routines.iter().find(|r| r.name == "top_spenders").unwrap();

    let snippet = db_query_lib::sqlgen::generate_call("poc", proc).unwrap();
    assert!(snippet.contains("/* min_total"), "{snippet}");

    // Fill the placeholders in, as a user would, and it runs.
    let filled = snippet
        .replace("/* min_total decimal(12,2) */", "0")
        .replace("/* max_rows int */", "10");
    let out = exec::run_script(&state, T, &filled, false, None)
        .await
        .unwrap();
    for st in &out.statements {
        assert!(
            !matches!(st.outcome, Outcome::Error { .. }),
            "filled-in call failed: {:?}\n{filled}",
            st.outcome
        );
    }
}

/// A no-argument procedure's snippet needs no editing at all.
#[tokio::test]
#[ignore]
async fn a_no_argument_call_runs_as_generated() {
    let state = connected().await;
    let routines = schema::list_routines(&state, C, "poc").await.unwrap();
    let proc = routines.iter().find(|r| r.name == "ping_poc").unwrap();
    let snippet = db_query_lib::sqlgen::generate_call("poc", proc).unwrap();
    let out = exec::run_script(&state, T, &snippet, false, None)
        .await
        .unwrap();
    for st in &out.statements {
        assert!(
            !matches!(st.outcome, Outcome::Error { .. }),
            "{:?}\n{snippet}",
            st.outcome
        );
    }
}

// ------------------------------------------------- Stage 3: export (E6)

/// Read a whole table as (columns, rows) through the real executor.
async fn fetch_all(
    state: &AppState,
    sql: &str,
) -> (Vec<db_query_lib::decode::ColumnMeta>, Vec<Vec<CellValue>>) {
    let out = exec::run_script(state, T, sql, false, None).await.unwrap();
    match &out.statements[0].outcome {
        Outcome::Rows { columns, rows, .. } => (columns.clone(), rows.clone()),
        other => panic!("expected rows from {sql}: {other:?}"),
    }
}

/// **E6.** Export a table as `CREATE TABLE` + `INSERT`s, drop it, replay the
/// script, and compare every row against what was there before.
///
/// The comparison includes a **BIGINT past 2^53 and a DECIMAL**, which is where
/// a generator that trusts the runtime type of a cell instead of its column
/// silently corrupts data — those arrive as text precisely so JavaScript cannot
/// round them.
#[tokio::test]
#[ignore]
async fn an_exported_table_recreates_itself_exactly() {
    let state = connected().await;

    exec::run_script(
        &state,
        T,
        "DROP TABLE IF EXISTS export_probe; \
         CREATE TABLE export_probe ( \
           id INT PRIMARY KEY, \
           big BIGINT, \
           money DECIMAL(20,4), \
           label VARCHAR(190), \
           when_ DATETIME, \
           flag TINYINT(1), \
           maybe VARCHAR(50) \
         );",
        false,
        None,
    )
    .await
    .unwrap();

    // Values chosen to break a naive generator: an integer JavaScript cannot
    // hold, a decimal it would round, quotes, a backslash, unicode, and a NULL.
    exec::run_script(
        &state,
        T,
        "INSERT INTO export_probe VALUES \
           (1, 9223372036854775807, 12345678901234.5678, 'it''s', '2026-09-05 14:30:00', 1, NULL), \
           (2, -9007199254740993, -0.0001, 'back\\\\slash', '1999-12-31 23:59:59', 0, 'here'), \
           (3, 0, 0.0000, 'héllo ★ 表 🔐', '2000-01-01 00:00:00', 1, '');",
        false,
        None,
    )
    .await
    .unwrap();

    let (columns, before) = fetch_all(&state, "SELECT * FROM export_probe ORDER BY id;").await;
    assert_eq!(before.len(), 3);

    // Exact DDL from the server, which is what makes this a true round trip
    // rather than a comparison against a widened approximation.
    let ddl = schema::table_ddl(&state, C, "poc", "export_probe")
        .await
        .unwrap();
    let script = db_query_lib::export::to_inserts(
        &columns,
        &before,
        &db_query_lib::export::InsertOptions {
            table: "export_probe".into(),
            db: Some("poc".into()),
            create_table: true,
            batch_size: 2,
        },
        Some(&ddl),
    )
    .unwrap();

    exec::run_script(&state, T, "DROP TABLE export_probe;", false, None)
        .await
        .unwrap();

    let out = exec::run_script(&state, T, &script, false, None)
        .await
        .unwrap();
    for st in &out.statements {
        assert!(
            !matches!(st.outcome, Outcome::Error { .. }),
            "exported script failed: {:?}\n--- script ---\n{script}",
            st.outcome
        );
    }

    let (after_cols, after) = fetch_all(&state, "SELECT * FROM export_probe ORDER BY id;").await;
    assert_eq!(
        after_cols.iter().map(|c| &c.name).collect::<Vec<_>>(),
        columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
    );
    assert_eq!(
        after, before,
        "the exported script did not reproduce the data"
    );

    exec::run_script(&state, T, "DROP TABLE IF EXISTS export_probe;", false, None)
        .await
        .unwrap();
}

/// The derived `CREATE TABLE` has no real table behind it, so the only way to
/// know it parses is to run it. `VARCHAR` without a length does not.
#[tokio::test]
#[ignore]
async fn a_derived_create_table_actually_parses() {
    let state = connected().await;
    let (columns, rows) = fetch_all(
        &state,
        "SELECT u.id, u.email, u.balance, u.created_at, o.total \
         FROM users u JOIN orders o ON o.user_id = u.id ORDER BY u.id, o.id;",
    )
    .await;
    assert!(!rows.is_empty(), "fixture should produce joined rows");

    let ddl = db_query_lib::export::derived_create_table(&columns, None, "derived_probe").unwrap();
    let script = format!(
        "DROP TABLE IF EXISTS derived_probe;\n{ddl}{}",
        db_query_lib::export::to_inserts(
            &columns,
            &rows,
            &db_query_lib::export::InsertOptions {
                table: "derived_probe".into(),
                db: None,
                create_table: true,
                batch_size: 50,
            },
            None,
        )
        .unwrap()
    );

    let out = exec::run_script(&state, T, &script, false, None)
        .await
        .unwrap();
    for st in &out.statements {
        assert!(
            !matches!(st.outcome, Outcome::Error { .. }),
            "derived schema did not parse: {:?}\n--- script ---\n{script}",
            st.outcome
        );
    }

    let (_, back) = fetch_all(&state, "SELECT COUNT(*) AS n FROM derived_probe;").await;
    assert_eq!(back[0][0], CellValue::Int(rows.len() as i64));

    exec::run_script(
        &state,
        T,
        "DROP TABLE IF EXISTS derived_probe;",
        false,
        None,
    )
    .await
    .unwrap();
}

/// `SHOW CREATE TABLE` on a view answers with a different column name.
#[tokio::test]
#[ignore]
async fn table_ddl_handles_a_view() {
    let state = connected().await;
    let ddl = schema::table_ddl(&state, C, "poc", "user_totals")
        .await
        .unwrap();
    assert!(ddl.to_uppercase().contains("VIEW"), "{ddl}");

    // MySQL stores a view as one normalised line, whatever it was written as.
    // That answer is correct and unreadable, which for "what does this view
    // actually select?" is close to no answer at all — so it is laid out on the
    // way to the tab. Asserted against the real server because the shape of
    // that one line is the server's choice, not ours.
    assert!(
        ddl.lines().count() > 4,
        "the view definition was not laid out:\n{ddl}"
    );
    let lower = ddl.to_lowercase();
    // The clauses this particular view has — it does not filter.
    for clause in ["\nselect", "\nfrom", "\ngroup by"] {
        assert!(
            lower.contains(clause),
            "{clause:?} did not start a line:\n{ddl}"
        );
    }
}

/// The other half of the same rule: a **table**'s definition arrives laid out
/// already, and must come back exactly as the server wrote it. Reformatting
/// what MySQL formatted would be a change with nothing to gain.
#[tokio::test]
#[ignore]
async fn table_ddl_leaves_the_servers_own_layout_alone() {
    let state = connected().await;
    let ddl = schema::table_ddl(&state, C, "poc", "users").await.unwrap();
    assert!(ddl.contains("CREATE TABLE"), "{ddl}");
    // The server indents columns by two spaces and keeps each on its own line.
    assert!(
        ddl.contains("\n  `id`"),
        "the column layout changed:\n{ddl}"
    );
    assert!(
        ddl.lines().all(|l| l.chars().count() <= 120),
        "a line was widened:\n{ddl}"
    );
}

// ------------------------------------- Stage 3: unbounded re-run (E8)

/// **E8's substance.** `big` holds 7500 rows and `MAX_ROWS` is 5000, so the
/// buffered path *must* truncate and the streaming path *must not*. If these
/// ever agree, one of them is broken.
#[tokio::test]
#[ignore]
async fn the_rerun_export_writes_more_rows_than_the_grid_can_hold() {
    let state = connected().await;
    let sql = "SELECT id, label, n FROM big ORDER BY id";

    // What the grid would show: capped, and flagged as capped.
    let shown = exec::run_script(&state, T, sql, true, None).await.unwrap();
    let Outcome::Rows {
        rows, truncated, ..
    } = &shown.statements[0].outcome
    else {
        panic!("expected rows")
    };
    assert_eq!(
        rows.len(),
        exec::MAX_ROWS,
        "the ceiling should have applied"
    );
    assert!(truncated, "a capped result must say so");

    // What the unbounded re-run writes: everything.
    let dir = std::env::temp_dir().join(format!("db-query-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.csv");

    let opts = csv_opts();
    let header_opts = opts.clone();
    let (written, bytes, cancelled) = exec::stream_to_file(
        &state,
        T,
        sql,
        &path,
        move |row, buf| {
            buf.push_str(&db_query_lib::export::csv_row(row, &opts));
            Ok(())
        },
        move |row| Ok(db_query_lib::export::csv_header(row, &header_opts)),
    )
    .await
    .unwrap();

    assert!(!cancelled);
    assert_eq!(written, 7500, "the streaming path must not be capped");
    assert!(
        written > exec::MAX_ROWS,
        "otherwise this test proves nothing"
    );

    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(bytes as usize, body.len());
    // 7500 data lines plus one header.
    assert_eq!(body.lines().count(), 7501, "header plus every row");
    assert!(
        body.starts_with("id,label,n"),
        "{:?}",
        &body[..40.min(body.len())]
    );
    assert!(body.contains("7500,row-7500,22500"), "last row missing");

    std::fs::remove_dir_all(&dir).ok();
}

fn csv_opts() -> db_query_lib::export::CsvOptions {
    // No BOM so the assertions above can compare against plain text.
    serde_json::from_str(
        r#"{"delimiter":",","crlf":false,"bom":false,"headers":true,"nullAs":"","formulaGuard":false}"#,
    )
    .unwrap()
}

/// A failed or cancelled export must not leave a file wearing the real name.
#[tokio::test]
#[ignore]
async fn a_failed_export_leaves_no_file_behind() {
    let state = connected().await;
    let dir = std::env::temp_dir().join(format!("db-query-export-fail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("nope.csv");

    let opts = csv_opts();
    let header_opts = opts.clone();
    let err = exec::stream_to_file(
        &state,
        T,
        "SELECT * FROM no_such_table_at_all",
        &path,
        move |row, buf| {
            buf.push_str(&db_query_lib::export::csv_row(row, &opts));
            Ok(())
        },
        move |row| Ok(db_query_lib::export::csv_header(row, &header_opts)),
    )
    .await
    .expect_err("a bad query should fail the export");

    assert!(!err.is_empty());
    assert!(
        !path.exists(),
        "a failed export left {} behind",
        path.display()
    );
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp file not cleaned up: {leftovers:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The connection must be usable immediately afterwards — a stream abandoned
/// mid-protocol is the classic way to poison one.
#[tokio::test]
#[ignore]
async fn the_connection_survives_a_streaming_export() {
    let state = connected().await;
    let dir = std::env::temp_dir().join(format!("db-query-export-reuse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("small.csv");

    let opts = csv_opts();
    let header_opts = opts.clone();
    exec::stream_to_file(
        &state,
        T,
        "SELECT id FROM big LIMIT 10",
        &path,
        move |row, buf| {
            buf.push_str(&db_query_lib::export::csv_row(row, &opts));
            Ok(())
        },
        move |row| Ok(db_query_lib::export::csv_header(row, &header_opts)),
    )
    .await
    .unwrap();

    let out = exec::run_script(&state, T, "SELECT 1 AS ok;", false, None)
        .await
        .unwrap();
    assert!(matches!(out.statements[0].outcome, Outcome::Rows { .. }));

    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------- query history, end to end
//
// `history.rs` is unit tested against synthetic entries. The question those
// cannot answer is whether *running a script actually records it* — the wiring
// between execution and the store, with real outcomes, real timings and a real
// current database.

/// A scratch config directory, so a test never touches the real history file.
fn history_dir(name: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("db-query-live-hist-{name}-{stamp}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
#[ignore]
async fn running_a_script_records_each_statement() {
    let state = connected().await;
    let dir = history_dir("record");

    let result = exec::run_script(
        &state,
        T,
        "SELECT 1 AS a; SELECT * FROM poc.users LIMIT 2;",
        false,
        None,
    )
    .await
    .unwrap();
    db_query_lib::remember(&dir, &state, T, &result).await;

    let hits = db_query_lib::history::search(&dir, None, None, 50);
    assert_eq!(hits.len(), 2, "both statements recorded");

    // Newest first, and the SQL is what was typed.
    assert!(hits[0].entry.sql.contains("users"));
    assert_eq!(hits[0].entry.status, "ok");
    assert_eq!(hits[0].entry.connection_id, C);
    assert_eq!(hits[0].entry.rows, Some(2), "the real row count is kept");

    session::disconnect(&state, C).await.unwrap();
}

/// A failure is often exactly the statement you are trying to find again, so it
/// is recorded with the server's own message.
#[tokio::test]
#[ignore]
async fn a_failed_statement_is_recorded_with_its_error() {
    let state = connected().await;
    let dir = history_dir("failed");

    let result = exec::run_script(&state, T, "SELECT * FROM poc.no_such_table", false, None)
        .await
        .unwrap();
    db_query_lib::remember(&dir, &state, T, &result).await;

    let hits = db_query_lib::history::search(&dir, None, None, 50);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry.status, "error");
    let message = hits[0].entry.error.clone().expect("an error message");
    assert!(message.contains("no_such_table"), "got {message}");

    session::disconnect(&state, C).await.unwrap();
}

/// Auto-LIMIT rewrites the statement before sending it. What gets recorded must
/// be what the **user wrote** — handing back our rewrite later, as though they
/// had typed it, is a small lie that compounds every time it is re-run.
#[tokio::test]
#[ignore]
async fn history_keeps_the_users_sql_not_our_rewrite() {
    let state = connected().await;
    let dir = history_dir("rewrite");

    let result = exec::run_script(&state, T, "SELECT * FROM poc.users", true, None)
        .await
        .unwrap();
    assert!(
        result.statements[0].effective_sql.is_some(),
        "this test is pointless unless auto-LIMIT actually rewrote the statement"
    );
    db_query_lib::remember(&dir, &state, T, &result).await;

    let hits = db_query_lib::history::search(&dir, None, None, 50);
    assert_eq!(hits[0].entry.sql, "SELECT * FROM poc.users");
    assert!(!hits[0].entry.sql.to_ascii_uppercase().contains("LIMIT"));

    session::disconnect(&state, C).await.unwrap();
}

/// The rule that matters most, proved against a real server: a password typed
/// into the editor never reaches the disk.
#[tokio::test]
#[ignore]
async fn a_credential_statement_is_never_written_down() {
    let state = connected().await;
    let dir = history_dir("secret");

    // Deliberately invalid so it fails rather than creating anything; the point
    // is what gets recorded, and a failed statement is recorded too.
    let result = exec::run_script(
        &state,
        T,
        "CREATE USER 'zz_hist_probe'@'%' IDENTIFIED BY 'zzSECRETzz-do-not-leak'; SELECT 1;",
        false,
        None,
    )
    .await
    .unwrap();
    db_query_lib::remember(&dir, &state, T, &result).await;

    let path = db_query_lib::history::path_in(&dir);
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        !raw.contains("zzSECRETzz-do-not-leak"),
        "a password reached the history file"
    );
    assert!(!raw.contains("IDENTIFIED BY"));
    // The innocent statement beside it is still recorded.
    assert!(raw.contains("SELECT 1"));

    // Clean up if the CREATE USER actually succeeded.
    let _ = exec::run_script(
        &state,
        T,
        "DROP USER IF EXISTS 'zz_hist_probe'@'%'",
        false,
        None,
    )
    .await;
    session::disconnect(&state, C).await.unwrap();
}

/// The assistant must be handed real columns, not "columns not loaded".
///
/// This is the bug the feature exists for: the prompt was built from whatever
/// the user happened to have expanded in the schema tree, so asking a question
/// before touching the tree described every table as having no columns — and a
/// model told that a table has no columns invents some.
///
/// Deliberately expands nothing first. A fresh connection is exactly the state
/// a user is in when they open the chat and type their first question.
#[tokio::test]
#[ignore]
async fn the_assistant_prompt_carries_columns_nobody_expanded() {
    let state = connected().await;

    let warmed = schema::warm_for_assistant(&state, C, "poc", schema::ASSISTANT_TABLE_BUDGET)
        .await
        .expect("warm failed");
    assert!(warmed.tables >= 4, "fixture should have tables: {warmed:?}");
    assert_eq!(
        warmed.detailed, warmed.tables,
        "the fixture is well under the budget, so every table should be detailed"
    );

    let server = session::server(&state, C).await.unwrap();
    let cache = server.schema_cache.lock().await;
    let rendered = db_query_lib::assistant::render_schema("poc", cache.get("poc").unwrap());

    assert!(
        !rendered.contains("columns not loaded"),
        "the model would have to guess these: {rendered}"
    );
    // Names and types, from the server rather than from the model's priors.
    assert!(rendered.contains("TABLE `users`"), "{rendered}");
    assert!(rendered.contains("email"), "{rendered}");
    assert!(rendered.contains("VIEW `user_totals`"), "{rendered}");

    // The rule this whole feature had to respect: introspection is not
    // execution. Nothing the user could have run was run.
    assert!(
        server.introspection_count.load(Ordering::SeqCst) > 0,
        "the warm-up should have introspected"
    );
}

/// Warming twice must not re-query: the cache is what makes the second question
/// as fast as the first.
#[tokio::test]
#[ignore]
async fn warming_a_second_time_costs_nothing() {
    let state = connected().await;
    schema::warm_for_assistant(&state, C, "poc", schema::ASSISTANT_TABLE_BUDGET)
        .await
        .unwrap();
    let server = session::server(&state, C).await.unwrap();
    let after_first = server.introspection_count.load(Ordering::SeqCst);

    schema::warm_for_assistant(&state, C, "poc", schema::ASSISTANT_TABLE_BUDGET)
        .await
        .unwrap();
    assert_eq!(
        server.introspection_count.load(Ordering::SeqCst),
        after_first,
        "the second warm-up went back to the server"
    );
}

/// **The idle-disconnect bug.**
///
/// `wait_timeout` defaults to eight hours, so the honest reproduction is to do
/// what the server does at the end of it: `KILL` the connection out from under
/// ourselves and then use it.
///
/// Before this, the exec connection healed (`ensure_exec` has pinged since
/// Stage 2) and the shared `meta` connection did not — so a session left alone
/// overnight could still run a query while the schema tree answered "Cannot
/// reach the server". That asymmetry is what this pins.
#[tokio::test]
#[ignore]
async fn the_meta_connection_heals_after_the_server_drops_it() {
    let state = connected().await;
    let server = session::server(&state, C).await.unwrap();

    // Warm it, so the failure below cannot be "it was never connected".
    let before = schema::list_tables(&state, C, "poc").await.unwrap();
    assert!(!before.is_empty());

    // The id of the meta connection itself, then kill it from another one.
    let victim: u64 = {
        let mut meta = server.mysql_meta().await.unwrap();
        sqlx::query("SELECT CONNECTION_ID() AS id")
            .fetch_one(&mut *meta)
            .await
            .unwrap()
            .try_get("id")
            .unwrap()
    };
    {
        let mut killer = server.mysql_killer().await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("KILL {victim}")))
            .execute(&mut *killer)
            .await
            .ok();
    }

    // Cache-busting: `list_tables` would answer from memory and prove nothing.
    schema::refresh(&state, C, "poc").await.unwrap();

    let after = schema::list_tables(&state, C, "poc")
        .await
        .expect("introspection must reconnect rather than report the server unreachable");
    assert_eq!(before.len(), after.len());

    // A new connection, not the corpse.
    let mut meta = server.mysql_meta().await.unwrap();
    let now: u64 = sqlx::query("SELECT CONNECTION_ID() AS id")
        .fetch_one(&mut *meta)
        .await
        .unwrap()
        .try_get("id")
        .unwrap();
    assert_ne!(now, victim, "the dead connection was handed back");
}

/// The same for the killer, because a cancel that cannot connect is a cancel
/// button that does nothing.
#[tokio::test]
#[ignore]
async fn the_killer_connection_heals_too() {
    let state = connected().await;
    let server = session::server(&state, C).await.unwrap();

    let victim: u64 = {
        let mut killer = server.mysql_killer().await.unwrap();
        sqlx::query("SELECT CONNECTION_ID() AS id")
            .fetch_one(&mut *killer)
            .await
            .unwrap()
            .try_get("id")
            .unwrap()
    };
    {
        let mut meta = server.mysql_meta().await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("KILL {victim}")))
            .execute(&mut *meta)
            .await
            .ok();
    }

    let mut killer = server.mysql_killer().await.expect("killer must reconnect");
    let now: u64 = sqlx::query("SELECT CONNECTION_ID() AS id")
        .fetch_one(&mut *killer)
        .await
        .unwrap()
        .try_get("id")
        .unwrap();
    assert_ne!(now, victim);
}
