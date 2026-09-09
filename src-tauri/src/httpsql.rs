//! The shared half of every SQL-over-HTTP engine.
//!
//! Elasticsearch is the first, and Snowflake's `POST /api/v2/statements` has
//! the same shape: post a statement, receive columns, rows and a pagination
//! handle. So the parts that are not Elasticsearch's — auth, JSON scalars into
//! `CellValue`, error extraction — live here, where the next one will find
//! them. See §3 of the Stage 11 tracker.

use serde::{Deserialize, Serialize};

use crate::decode::{type_hint, CellValue, ColumnMeta};

/// How to prove who we are. Three shapes, because a local cluster wants none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Auth {
    /// The default, because a local cluster usually wants nothing.
    #[default]
    None,
    Basic {
        user: String,
    },
    /// Elasticsearch's `Authorization: ApiKey <base64>`.
    ApiKey,
}

impl Auth {
    /// Attach credentials, if this scheme uses any.
    pub fn apply(
        &self,
        req: reqwest::RequestBuilder,
        secret: Option<&str>,
    ) -> reqwest::RequestBuilder {
        match (self, secret) {
            (Auth::Basic { user }, Some(pass)) => req.basic_auth(user, Some(pass)),
            (Auth::ApiKey, Some(key)) => req.header("authorization", format!("ApiKey {key}")),
            // Including `Basic`/`ApiKey` with no secret: sending an empty
            // credential is worse than sending none, because the server's error
            // then blames the credential rather than its absence.
            _ => req,
        }
    }
}

/// One JSON scalar as a grid cell.
///
/// Objects and arrays become their JSON text rather than being flattened: a
/// nested document is a real value, and the cell viewer added in Stage 6 can
/// already show it. Losing it to "[object]" would be the worse trade.
pub fn cell_from_json(v: &serde_json::Value) -> CellValue {
    match v {
        serde_json::Value::Null => CellValue::Null,
        serde_json::Value::Bool(b) => CellValue::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => CellValue::Int(i),
            None => n
                .as_f64()
                .map(CellValue::Float)
                .unwrap_or_else(|| CellValue::Text(n.to_string())),
        },
        serde_json::Value::String(s) => CellValue::Text(s.clone()),
        other => CellValue::Text(other.to_string()),
    }
}

/// Column metadata from a `{name, type}` pair.
pub fn column_from_json(v: &serde_json::Value) -> ColumnMeta {
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("?")
        .to_string();
    let sql_type = v
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown")
        .to_string();
    ColumnMeta {
        name,
        type_hint: type_hint(&sql_type),
        sql_type,
    }
}

/// Pull a human-readable reason out of an error body.
///
/// Elasticsearch nests the useful sentence at `error.root_cause[0].reason`, with
/// `error.reason` as the fallback; other servers use `error.message` or a bare
/// `message`. Trying each in turn beats showing a page of JSON, and falling back
/// to the raw body beats showing nothing.
pub fn error_message(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            let e = v.get("error");
            e.and_then(|e| e.get("root_cause"))
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("reason"))
                .and_then(|r| r.as_str())
                .or_else(|| e.and_then(|e| e.get("reason")).and_then(|r| r.as_str()))
                .or_else(|| e.and_then(|e| e.get("message")).and_then(|m| m.as_str()))
                .or_else(|| v.get("message").and_then(|m| m.as_str()))
                .map(str::to_owned)
                // An `error` that is itself a string, which older versions send.
                .or_else(|| e.and_then(|e| e.as_str()).map(str::to_owned))
        })
        .unwrap_or_else(|| body.chars().take(400).collect());

    match status {
        401 => format!("Authentication failed: {detail}"),
        403 => format!("Not permitted: {detail}"),
        404 => format!("Not found: {detail}"),
        _ => detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::TypeHint;

    #[test]
    fn json_scalars_become_the_grid_types_the_app_already_has() {
        assert_eq!(cell_from_json(&serde_json::json!(null)), CellValue::Null);
        assert_eq!(
            cell_from_json(&serde_json::json!(true)),
            CellValue::Bool(true)
        );
        assert_eq!(cell_from_json(&serde_json::json!(7)), CellValue::Int(7));
        assert_eq!(
            cell_from_json(&serde_json::json!(1.5)),
            CellValue::Float(1.5)
        );
        assert_eq!(
            cell_from_json(&serde_json::json!("hi")),
            CellValue::Text("hi".into())
        );
    }

    /// An integer must not arrive as a float — the same rule `CellValue`'s
    /// untagged ordering exists to enforce.
    #[test]
    fn a_whole_number_stays_an_integer() {
        assert_eq!(cell_from_json(&serde_json::json!(1)), CellValue::Int(1));
    }

    /// A nested document is a real value. The cell viewer can show it; "[object]"
    /// would throw it away.
    #[test]
    fn nested_documents_keep_their_json() {
        let v = cell_from_json(&serde_json::json!({"a": [1, 2]}));
        match v {
            CellValue::Text(s) => {
                assert!(s.contains("\"a\""), "{s}");
                assert!(s.contains('2'), "{s}");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn column_metadata_carries_the_servers_own_type_name() {
        let c = column_from_json(&serde_json::json!({"name": "total", "type": "long"}));
        assert_eq!(c.name, "total");
        assert_eq!(c.sql_type, "long");
        // The hint drives alignment in the grid; "long" must read as numeric.
        assert_eq!(c.type_hint, TypeHint::Numeric);
    }

    #[test]
    fn an_elasticsearch_error_shows_the_reason_not_the_json() {
        let body = r#"{"error":{"root_cause":[{"type":"verification_exception",
            "reason":"Unknown index [nope]"}],"type":"verification_exception",
            "reason":"Unknown index [nope]"},"status":400}"#;
        assert_eq!(error_message(400, body), "Unknown index [nope]");
    }

    #[test]
    fn other_error_shapes_still_produce_a_sentence() {
        assert_eq!(
            error_message(400, r#"{"error":{"message":"bad query"}}"#),
            "bad query"
        );
        assert_eq!(error_message(400, r#"{"message":"nope"}"#), "nope");
        assert_eq!(error_message(400, r#"{"error":"legacy"}"#), "legacy");
    }

    #[test]
    fn a_non_json_body_still_produces_something_readable() {
        let m = error_message(502, "<html>Bad Gateway</html>");
        assert!(m.contains("Bad Gateway"), "{m}");
    }

    #[test]
    fn status_codes_that_need_naming_are_named() {
        assert!(error_message(401, "{}").starts_with("Authentication failed"));
        assert!(error_message(404, "{}").starts_with("Not found"));
    }

    /// Sending an empty credential is worse than sending none: the server then
    /// blames the credential rather than its absence.
    /// Tests build their own clients, so they install the provider the app
    /// installs at startup. Idempotent, so several tests may call it.
    fn client() -> reqwest::Client {
        crate::install_tls();
        reqwest::Client::new()
    }

    #[test]
    fn a_missing_secret_attaches_no_header() {
        let client = client();
        let built = Auth::ApiKey
            .apply(client.post("http://localhost/_sql"), None)
            .build()
            .unwrap();
        assert!(built.headers().get("authorization").is_none());
    }

    #[test]
    fn an_api_key_is_sent_the_way_elasticsearch_expects() {
        let client = client();
        let built = Auth::ApiKey
            .apply(client.post("http://localhost/_sql"), Some("abc123"))
            .build()
            .unwrap();
        assert_eq!(built.headers()["authorization"], "ApiKey abc123");
    }
}
