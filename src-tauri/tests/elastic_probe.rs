//! The Elasticsearch connect probe, against canned HTTP.
//!
//! Reported in use, 2026-10-19: a cluster account with index privileges and no
//! cluster privileges could not connect at all, because the probe asked `GET /`
//! for a version and `GET /` needs the `monitor` cluster privilege. Everything
//! the app actually does — `SELECT`, `SHOW TABLES`, `SHOW CATALOGS`,
//! `DESCRIBE` — needs none of it.
//!
//! These are ordinary tests, not `#[ignore]`d ones, on purpose: the live suite
//! runs against a fixture with security switched off, so **no live test can
//! ever produce a 403** and the branch that matters would have no cover. The
//! bodies here are copied from a real 8.15.0 cluster with
//! `{"cluster": [], "indices": [{"names": ["orders*"], "privileges": ["all"]}]}`
//! (see `mise run es-secure-up`), so what is canned is measured, not imagined.
//!
//!     cargo test --test elastic_probe

use db_query_lib::elastic::ElasticEngine;
use db_query_lib::httpsql::Auth;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A cluster that answers with exactly these replies, in order, and remembers
/// what it was asked. One connection per reply, closed after it — so a request
/// the probe does not make is a connection nobody accepts, and the count of
/// what was seen is the count of what was sent.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn engine(&self) -> ElasticEngine {
        ElasticEngine::new(&self.base, Auth::None, None)
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

async fn fake(replies: Vec<(u16, &'static str)>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let recorded = Arc::clone(&seen);
    tokio::spawn(async move {
        for (status, body) in replies {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };

            // The whole request, not whatever arrived first: headers to the
            // blank line, then exactly `content-length` more. A test that reads
            // one packet and hopes is a test that fails on a slow day.
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                let Some(head) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&buf[..head]).to_lowercase();
                let want: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= head + 4 + want {
                    break;
                }
            }
            recorded
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf).to_string());

            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });

    Fake { base, seen }
}

/// The 403 a cluster sends for `GET /` when the account holds no cluster
/// privileges — copied from the fixture, which is where the user's own report
/// came from word for word.
const ROOT_FORBIDDEN: &str = r#"{"error":{"root_cause":[{"type":"security_exception","reason":"action [cluster:monitor/main] is unauthorized for user [restricted] with effective roles [index_admin], this action is granted by the cluster privileges [monitor,manage,all]"}],"type":"security_exception","reason":"action [cluster:monitor/main] is unauthorized for user [restricted] with effective roles [index_admin], this action is granted by the cluster privileges [monitor,manage,all]"},"status":403}"#;

/// The 403 `_sql` sends to an account that cannot read any index.
const SQL_FORBIDDEN: &str = r#"{"error":{"root_cause":[{"type":"security_exception","reason":"action [indices:data/read/sql] is unauthorized for user [nobody] with effective roles [nothing_at_all], this action is granted by the index privileges [read,all]"}],"type":"security_exception","reason":"action [indices:data/read/sql] is unauthorized for user [nobody] with effective roles [nothing_at_all], this action is granted by the index privileges [read,all]"},"status":403}"#;

const ROOT_OK: &str = r#"{"name":"n","cluster_name":"docker-cluster","version":{"number":"8.15.0","build_flavor":"default"},"tagline":"You Know, for Search"}"#;

const SELECT_ONE: &str = r#"{"columns":[{"name":"1","type":"integer"}],"rows":[[1]]}"#;

/// The ordinary path: one request, and the version it reports.
#[tokio::test]
async fn a_root_that_answers_gives_the_version_in_one_request() {
    let f = fake(vec![(200, ROOT_OK)]).await;
    assert_eq!(f.engine().probe().await.unwrap().as_deref(), Some("8.15.0"));
    let seen = f.requests();
    assert_eq!(seen.len(), 1, "the ordinary path must stay one round trip");
    assert!(seen[0].starts_with("GET / HTTP/1.1"), "{}", seen[0]);
}

/// A root response with no version number is the same state as a root response
/// we may not read: there, and not saying. One encoding, not two.
#[tokio::test]
async fn a_root_without_a_version_number_is_unknown_not_a_failure() {
    let f = fake(vec![(200, "{}")]).await;
    assert_eq!(f.engine().probe().await.unwrap(), None);
}

/// **The reported bug.** A 403 on the root is not a failed connection: it is an
/// account that may work without being allowed to ask what it is working on.
#[tokio::test]
async fn a_forbidden_root_connects_anyway_and_asks_the_query_the_app_runs() {
    let f = fake(vec![(403, ROOT_FORBIDDEN), (200, SELECT_ONE)]).await;

    assert_eq!(
        f.engine().probe().await,
        Ok(None),
        "an index-only account must be able to connect"
    );

    let seen = f.requests();
    assert_eq!(seen.len(), 2, "the root, then the fallback");
    // The fallback asks `_sql` — the thing the engine is for — rather than
    // another endpoint that happens to answer.
    assert!(seen[1].starts_with("POST /_sql?format=json"), "{}", seen[1]);
    assert!(seen[1].contains("SELECT 1"), "{}", seen[1]);
}

/// And it is a real check, not a formality: an account that cannot query either
/// is still refused — with the privilege worth asking for, not with
/// `cluster:monitor/main`, which was never the problem.
#[tokio::test]
async fn an_account_that_cannot_query_is_refused_with_the_privilege_it_needs() {
    let f = fake(vec![(403, ROOT_FORBIDDEN), (403, SQL_FORBIDDEN)]).await;

    let err = f.engine().probe().await.expect_err("must not connect");
    assert!(err.starts_with("Not permitted: "), "{err}");
    assert!(err.contains("indices:data/read/sql"), "{err}");
    assert!(err.contains("index privileges [read,all]"), "{err}");
    assert!(
        !err.contains("cluster:monitor"),
        "the message must name what this account is short of: {err}"
    );
}

/// A wrong password is **401**, measured, and stays fatal. If 403 had been read
/// as "credentials are fine" by accident rather than by measurement, this is
/// the test that would have caught it.
#[tokio::test]
async fn a_rejected_password_is_still_a_failed_connection() {
    let body = r#"{"error":{"root_cause":[{"type":"security_exception","reason":"unable to authenticate user [restricted] for REST request [/]"}],"type":"security_exception","reason":"unable to authenticate user [restricted] for REST request [/]"},"status":401}"#;
    let f = fake(vec![(401, body)]).await;

    let err = f.engine().probe().await.expect_err("must not connect");
    assert!(err.starts_with("Authentication failed: "), "{err}");
    assert!(err.contains("unable to authenticate"), "{err}");
    assert_eq!(
        f.requests().len(),
        1,
        "a bad password must not be retried against _sql"
    );
}

/// Anything else the cluster says is still a failure, unchanged.
#[tokio::test]
async fn a_cluster_that_errors_is_a_failed_connection() {
    let f = fake(vec![(503, r#"{"error":{"reason":"no master"}}"#)]).await;
    assert_eq!(f.engine().probe().await, Err("no master".to_string()));
}

/// Nothing listening is nothing to connect to, and the message says where.
#[tokio::test]
async fn an_address_with_nothing_on_it_says_so() {
    // Bound, then dropped: a port nobody is on, without guessing a number.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let engine = ElasticEngine::new(&format!("http://127.0.0.1:{port}"), Auth::None, None);
    let err = engine.probe().await.expect_err("must not connect");
    assert!(
        err.starts_with("Could not reach http://127.0.0.1:"),
        "{err}"
    );
}
