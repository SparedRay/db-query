//! Connecting to a cluster **with security on**, as three different accounts.
//!
//! The ordinary fixture (`es-up`) runs with `xpack.security.enabled=false`, so
//! it cannot produce a 403 and cannot tell one account from another. That is
//! how a privilege bug reached a user: an account with index privileges and no
//! cluster privileges could not connect, because the probe asked `GET /` for a
//! version and `GET /` needs `monitor` (Stage 11 §12).
//!
//!     mise run es-secure-up
//!     mise run test-es-secure
//!
//! Ignored by default; they need the secured fixture, whose addresses and
//! passwords are the task's. `tests/elastic_probe.rs` covers the same branches
//! against canned HTTP, so this suite is the proof that the cans are honest.

use db_query_lib::httpsql::Auth;
use db_query_lib::session::{self, AppState, ConnProfile, EngineKind};

fn url() -> String {
    std::env::var("ES_SECURE_URL").unwrap_or_else(|_| "http://localhost:9201".into())
}

fn profile(id: &str, user: &str) -> ConnProfile {
    ConnProfile {
        id: id.into(),
        name: format!("secured as {user}"),
        colour: "#3b82f6".into(),
        host: String::new(),
        port: 0,
        user: String::new(),
        database: None,
        allow_invalid_certs: false,
        kind: EngineKind::Elasticsearch,
        url: url(),
        auth: Auth::Basic { user: user.into() },
        no_password: false,
        read_only: false,
        dictionary: None,
    }
}

/// **The reported bug, against a real cluster.** An account holding index
/// privileges and nothing else connects, and the version it is not allowed to
/// read comes back as "not known" rather than as a refusal.
#[tokio::test]
#[ignore]
async fn an_index_only_account_connects_without_a_version() {
    let state = AppState::default();
    let info = session::connect(
        &state,
        profile("es-restricted", "restricted"),
        "restricted-9201".into(),
    )
    .await
    .expect("an index-only account must be able to connect — is the fixture up?");

    assert_eq!(
        info.server_version, None,
        "this account cannot read the root, so there is no version to report"
    );
    // And it is a working connection, not a token one: the catalog list is a
    // `SHOW CATALOGS`, which needs no cluster privilege either.
    assert!(
        info.databases.iter().any(|d| !d.is_empty()),
        "got {:?}",
        info.databases
    );
}

/// The same account runs a query through the ordinary path — which is the whole
/// point of letting it connect.
#[tokio::test]
#[ignore]
async fn an_index_only_account_can_then_query() {
    let state = AppState::default();
    session::connect(
        &state,
        profile("es-restricted", "restricted"),
        "restricted-9201".into(),
    )
    .await
    .expect("connect failed — is the fixture up?");
    session::open_tab(&state, "es-restricted", "tab1")
        .await
        .unwrap();

    let result = db_query_lib::exec::run_script(
        &state,
        "tab1",
        "SELECT \"user\", total FROM orders ORDER BY total DESC",
        true,
        None,
    )
    .await
    .expect("the query this account is allowed to run");

    let rows = match &result.statements[0].outcome {
        db_query_lib::exec::Outcome::Rows { rows, .. } => rows.len(),
        other => panic!("expected rows, got {other:?}"),
    };
    assert_eq!(rows, 2, "the secured fixture seeds two documents");
}

/// An account that cannot query either is refused — **with the privilege it is
/// short of**, which is an index privilege and never `cluster:monitor`.
#[tokio::test]
#[ignore]
async fn an_account_with_no_privileges_is_refused_usefully() {
    let state = AppState::default();
    let err = session::connect(&state, profile("es-nobody", "nobody"), "nobody-9201".into())
        .await
        .expect_err("an account that cannot query must not connect");

    assert!(err.contains("indices:data/read/sql"), "{err}");
    assert!(
        !err.contains("cluster:monitor"),
        "the message must name what this account is short of: {err}"
    );
    assert!(
        state.connections.lock().await.is_empty(),
        "a failed connection must leave no trace"
    );
}

/// A wrong password is still a wrong password. 403 is not read as "the
/// credentials are fine" by accident: 401 arrives for a bad one, measured.
#[tokio::test]
#[ignore]
async fn a_wrong_password_is_still_a_failed_connection() {
    let state = AppState::default();
    let err = session::connect(
        &state,
        profile("es-restricted", "restricted"),
        "not-the-password".into(),
    )
    .await
    .expect_err("a wrong password must not connect");

    assert!(err.starts_with("Authentication failed: "), "{err}");
}

/// And the privileged account is unchanged: a version, in one request.
#[tokio::test]
#[ignore]
async fn a_privileged_account_still_reports_the_version() {
    let state = AppState::default();
    let info = session::connect(
        &state,
        profile("es-elastic", "elastic"),
        "measure-me-9201".into(),
    )
    .await
    .expect("connect failed — is the fixture up?");

    assert!(
        info.server_version
            .as_deref()
            .is_some_and(|v| !v.is_empty()),
        "got {:?}",
        info.server_version
    );
}
