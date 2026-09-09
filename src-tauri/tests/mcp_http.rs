//! The MCP server over a real socket.
//!
//! The unit tests in `mcp.rs` decide the *rules* — which `Origin` counts as
//! local, which token is accepted. These prove the rules are actually reached:
//! that something is listening, that it refuses what it says it refuses, and —
//! the one that cannot be proved by reading code — that **off means off**.
//!
//! Not `#[ignore]`d: it binds port 0 on loopback and needs no database, no
//! keychain and no network. `start_with_token` exists so the keychain stays out
//! of it; a test of the lock that CI cannot run is not a test of the lock.

use std::time::Duration;

use db_query_lib::mcp::{self, McpState};
use db_query_lib::session::AppState;
use tauri::Manager as _;

/// An app with the two pieces of state the handler reaches for, and the MCP
/// server listening on a port the OS chose.
async fn serve() -> (
    tauri::App<tauri::test::MockRuntime>,
    McpState,
    String,
    String,
) {
    let app = tauri::test::mock_app();
    app.manage(AppState::default());
    let state = McpState::default();
    let token = "test-token-not-from-the-keychain".to_string();

    let status = mcp::start_with_token(&app.handle().clone(), &state, 0, token.clone())
        .await
        .expect("bind on an OS-chosen loopback port");
    let url = status.url.expect("a running server reports its URL");
    (app, state, url, token)
}

fn client() -> reqwest::Client {
    // The same trap the app itself fell into: `reqwest` is built with
    // `rustls-no-provider`, so somebody has to choose one or `Client::new`
    // panics. Sharing the app's own function keeps the test's TLS setup
    // identical to the app's rather than merely similar. It is idempotent.
    db_query_lib::install_tls();
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("a plain HTTP client")
}

/// Pull the tool names out of a Streamable HTTP response.
///
/// The transport answers in SSE frames by default, so the JSON arrives on
/// `data:` lines rather than as the whole body.
fn tool_names(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .find_map(|v| {
            let tools = v.get("result")?.get("tools")?.as_array()?.clone();
            Some(
                tools
                    .iter()
                    .filter_map(|t| Some(t.get("name")?.as_str()?.to_string()))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default()
}

/// The MCP `initialize` request, which is the first thing any client sends.
fn initialize() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": mcp::PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp_http test", "version": "0"}
        }
    })
}

#[tokio::test]
async fn a_request_with_no_token_is_refused_and_says_so() {
    let (_app, state, url, _token) = serve().await;

    let res = client()
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .json(&initialize())
        .send()
        .await
        .expect("the server is listening");

    assert_eq!(res.status(), 401);
    assert_eq!(
        res.headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer"),
        "a 401 has to tell the client what scheme to use"
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("bearer token"), "unhelpful refusal: {body}");

    mcp::stop(&state).await;
}

#[tokio::test]
async fn a_wrong_token_is_refused() {
    let (_app, state, url, _token) = serve().await;

    let res = client()
        .post(&url)
        .header("authorization", "Bearer wrong")
        .header("accept", "application/json, text/event-stream")
        .json(&initialize())
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    mcp::stop(&state).await;
}

/// The spec's one MUST for this transport. Without it, a page you happen to
/// visit can drive the server through DNS rebinding.
#[tokio::test]
async fn a_foreign_origin_is_refused_even_with_the_right_token() {
    let (_app, state, url, token) = serve().await;

    let res = client()
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("origin", "https://evil.example")
        .header("accept", "application/json, text/event-stream")
        .json(&initialize())
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 403);
    // The wording is asserted on purpose, and it is `gate`'s. rmcp refuses this
    // too — deleting our check leaves the 403 intact and only changes the body
    // to "Origin header is not allowed" (verified by breaking it). Checking the
    // status alone would therefore pass with our own validation gone.
    let body = res.text().await.unwrap();
    assert!(body.contains("local origins"), "unhelpful refusal: {body}");

    mcp::stop(&state).await;
}

#[tokio::test]
async fn a_wrong_path_says_where_the_server_is() {
    let (_app, state, url, token) = serve().await;
    let root = url.trim_end_matches(mcp::ENDPOINT_PATH).to_string();

    let res = client()
        .get(&root)
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);
    assert!(res.text().await.unwrap().contains(mcp::ENDPOINT_PATH));

    mcp::stop(&state).await;
}

/// A real handshake, and then the tool list a client would see. The unit test
/// asserts what the router holds; this asserts what comes back over the wire.
#[tokio::test]
async fn a_client_handshake_lists_exactly_the_four_tools() {
    let (_app, state, url, token) = serve().await;
    let http = client();

    let res = http
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json, text/event-stream")
        .json(&initialize())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "initialize was refused");
    let session = res
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = res.text().await.unwrap();
    assert!(
        body.contains("db-query"),
        "initialize did not name the server: {body}"
    );

    // The notification that completes the handshake. A server that skipped it
    // would answer `tools/list` anyway, so it is sent to keep this honest
    // about being a client.
    let mut notify = http
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    if let Some(id) = &session {
        notify = notify.header("mcp-session-id", id.clone());
    }
    notify.send().await.unwrap();

    let mut list = http
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }));
    if let Some(id) = &session {
        list = list.header("mcp-session-id", id.clone());
    }
    let body = list.send().await.unwrap().text().await.unwrap();

    // The names, not the whole body: every description here says the word
    // "executed" — in the sentence promising it is not — so a substring search
    // over the payload would fail on the very text that makes the promise.
    let mut names = tool_names(&body);
    names.sort();
    let mut expected: Vec<String> = mcp::TOOL_NAMES.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(names, expected, "the advertised tool list changed: {body}");

    mcp::stop(&state).await;
}

/// M4. Proved by failing to connect, not by reading the code — the whole
/// reason a switch labelled "off" is worth anything.
#[tokio::test]
async fn off_means_nothing_is_listening() {
    let (_app, state, url, token) = serve().await;

    // It answers now.
    assert!(client()
        .post(&url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json, text/event-stream")
        .json(&initialize())
        .send()
        .await
        .is_ok());

    mcp::stop(&state).await;
    // The listener is dropped on the accept loop's next wake-up.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let after = client().post(&url).json(&initialize()).send().await;
    assert!(
        after.is_err(),
        "something still answered on {url} after stop: {after:?}"
    );
    assert!(!state.status().await.running);
}

/// Changing the port must not leave the old one open, which is the failure a
/// naive "start again" would ship.
#[tokio::test]
async fn restarting_releases_the_previous_port() {
    let (app, state, first_url, token) = serve().await;

    mcp::start_with_token(&app.handle().clone(), &state, 0, token.clone())
        .await
        .expect("a second start");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let old = client().post(&first_url).json(&initialize()).send().await;
    assert!(old.is_err(), "the first port is still open: {old:?}");

    let status = state.status().await;
    assert!(status.running);
    assert_ne!(status.url.as_deref(), Some(first_url.as_str()));

    mcp::stop(&state).await;
}
