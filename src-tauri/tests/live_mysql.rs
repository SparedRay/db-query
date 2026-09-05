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
use db_query_lib::session::{self, AppState, ConnConfig};

fn config() -> ConnConfig {
    ConnConfig {
        host: "127.0.0.1".into(),
        port: 3306,
        user: "root".into(),
        password: "devpassword".into(),
        database: Some("poc".into()),
        // The container ships a self-signed cert, so strict verification would
        // (correctly) refuse it. See `refuses_a_self_signed_cert_by_default`.
        allow_invalid_certs: true,
    }
}

async fn connected() -> AppState {
    let state = AppState::default();
    session::connect(&state, config())
        .await
        .expect("connect failed — is the fixture container up?");
    state
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
    let info = session::connect(&state, config()).await.unwrap();
    assert!(
        info.server_version.starts_with('8'),
        "got {}",
        info.server_version
    );
    assert!(info.databases.iter().any(|d| d == "poc"));
    assert!(info.connection_id > 0);
    session::disconnect(&state).await.unwrap();
}

#[tokio::test]
#[ignore]
async fn m1_wrong_password_is_readable_and_does_not_panic() {
    let state = AppState::default();
    let mut bad = config();
    bad.password = "definitely-wrong".into();
    let err = session::connect(&state, bad).await.unwrap_err();
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
    let err = session::connect(&state, strict).await.unwrap_err();
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
    let r = exec::run_script(&state, "SELECT * FROM types_zoo", true, None)
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
    let r = exec::run_script(&state, "SELECT 'a;b' AS v", true, None)
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
    let r = exec::run_script(&state, "SHOW TABLES; DESCRIBE users", true, None)
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
    let tables = schema::list_tables(&state, "poc").await.unwrap();
    assert!(tables
        .iter()
        .any(|t| t.name == "users" && t.kind == "BASE TABLE"));
    assert!(tables
        .iter()
        .any(|t| t.name == "user_totals" && t.kind == "VIEW"));

    let cols = schema::list_columns(&state, "poc", "users").await.unwrap();
    assert_eq!(cols.len(), 5);
    let email = cols.iter().find(|c| c.name == "email").unwrap();
    assert!(!email.nullable);
    assert_eq!(email.key.as_deref(), Some("UNI"));

    // Second call must be served from cache. Proven by dropping the exec
    // connection: a cached read still succeeds, an uncached one cannot.
    session::drop_exec_for_test(&state).await;
    assert_eq!(
        schema::list_tables(&state, "poc").await.unwrap().len(),
        tables.len()
    );
    assert_eq!(
        schema::list_columns(&state, "poc", "users")
            .await
            .unwrap()
            .len(),
        5
    );
}

// ------------------------------------------------------------------ M5

#[tokio::test]
#[ignore]
async fn m5_auto_limit_truncates_a_big_table_and_reports_the_rewrite() {
    let state = connected().await;
    let r = exec::run_script(&state, "SELECT * FROM big", true, None)
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
    let r = exec::run_script(&state, "SELECT * FROM big", false, None)
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
        session::cancel_query(&killer).await.expect("cancel failed");
    });

    let started = Instant::now();
    let r = exec::run_script(&state, "SELECT SLEEP(30)", true, None)
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
        session::cancel_query(&killer).await.ok();
    });
    let r = exec::run_script(&state, "SELECT SLEEP(30); SELECT 999", true, None)
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
    exec::run_script(&state, "USE mysql; SELECT 1", true, None)
        .await
        .unwrap();
    let ctl = state.ctl.lock().await;
    assert_eq!(ctl.as_ref().unwrap().current_db.as_deref(), Some("mysql"));
}

// --------------------------------------------------------------- Phase 6

#[tokio::test]
#[ignore]
async fn timeout_kills_a_slow_query_and_is_reported_separately_from_cancel() {
    use std::time::{Duration, Instant};
    let state = connected().await;

    let started = Instant::now();
    let r = exec::run_script(&state, "SELECT SLEEP(30)", true, Some(1))
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
    let r = exec::run_script(&state, "SELECT SLEEP(0.2)", true, None)
        .await
        .unwrap();
    assert!(!r.timed_out);
    let r = exec::run_script(&state, "SELECT SLEEP(0.2)", true, Some(0))
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
    let timed = exec::run_script(&state, "SELECT SLEEP(30)", true, Some(1))
        .await
        .unwrap();
    assert!(timed.timed_out);

    let after = exec::run_script(&state, "SELECT 1 + 1 AS two", true, None)
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
    session::use_database(&state, "poc").await.unwrap();
    // Warm the cache the way the sidebar would.
    schema::list_columns(&state, "poc", "users").await.unwrap();

    let cached = {
        let g = state.ctl.lock().await;
        let ctl = g.as_ref().unwrap();
        ctl.schema_cache["poc"]
            .columns
            .iter()
            .map(|(t, c)| {
                (
                    t.to_ascii_lowercase(),
                    c.iter().map(|x| x.name.to_ascii_lowercase()).collect(),
                )
            })
            .collect::<db_query_lib::lint::LintSchema>()
    };

    let good = db_query_lib::lint::lint("SELECT email FROM users", &cached);
    assert!(good.is_empty(), "real column flagged: {good:?}");

    let bad = db_query_lib::lint::lint("SELECT emial FROM users", &cached);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].message.contains("no column `emial`"));
}
