//! An assistant that writes SQL for you to read.
//!
//! # The rule this is built around
//!
//! **It proposes; you execute.** Since Stage 0 this project has never run a
//! statement the user did not ask it to run, and an assistant is exactly the
//! feature that would erode that quietly. So the model is given **no tools at
//! all** — it cannot query, cannot connect, cannot act. It answers in prose and
//! writes SQL into the editor, where the same Run button and the same review
//! step apply as to anything you typed yourself.
//!
//! That is also why there is no tool-use loop here: without execution, the
//! whole request is one streamed completion.
//!
//! # What leaves the machine
//!
//! The **shape** of your schema — table names, column names and types for the
//! active database — plus what you type in the chat. **No row data, ever**, and
//! nothing at all until the feature is switched on with a key. Both facts are
//! stated in the UI rather than buried here.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::schema::DbSchema;

/// The keychain account the API key lives under.
///
/// Namespaced so it cannot collide with a connection id, which is a UUID. Like
/// every other secret in this app it lives in the OS credential store, never in
/// a config file.
pub const KEY_ID: &str = "assistant:anthropic";

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2023-06-01";
/// Anthropic's most capable model. Chosen deliberately: writing correct SQL
/// against an unfamiliar schema is the kind of task where the difference shows.
pub const MODEL: &str = "claude-opus-5";
/// Streaming, so a long answer cannot hit a request timeout.
pub const MAX_TOKENS: u32 = 16_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// "user" or "assistant".
    pub role: String,
    pub content: String,
}

/// What the frontend receives while a reply is being written.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamEvent {
    /// A summary of the model's reasoning, shown dimmed. Requested explicitly:
    /// without it a thinking model looks like a long pause before anything.
    Thinking {
        delta: String,
    },
    Text {
        delta: String,
    },
    Done {
        stop_reason: Option<String>,
    },
    /// The request failed. Carried as an event rather than thrown so a partial
    /// answer already on screen is not discarded.
    Failed {
        message: String,
    },
}

// ------------------------------------------------------------- schema context

/// Render a database's cached schema as compact text for the system prompt.
///
/// Only names and types — the shape, never the contents. Tables whose columns
/// have not been expanded yet appear by name alone, which is honest: it tells
/// the model the table exists without inventing columns for it.
pub fn render_schema(db: &str, schema: &DbSchema) -> String {
    let mut out = String::new();
    out.push_str(&format!("Database `{db}`\n"));

    let tables = schema.tables.as_deref().unwrap_or_default();
    if tables.is_empty() {
        out.push_str("(no tables have been loaded yet)\n");
        return out;
    }

    for t in tables {
        let kind = if t.kind.to_ascii_uppercase().contains("VIEW") {
            "VIEW"
        } else {
            "TABLE"
        };
        match schema.columns.get(&t.name) {
            Some(cols) if !cols.is_empty() => {
                out.push_str(&format!("{kind} `{}` (", t.name));
                let rendered: Vec<String> = cols
                    .iter()
                    .map(|c| {
                        let mut s = format!("{} {}", c.name, c.data_type);
                        if !c.nullable {
                            s.push_str(" NOT NULL");
                        }
                        match c.key.as_deref() {
                            Some("PRI") => s.push_str(" PK"),
                            Some("UNI") => s.push_str(" UNIQUE"),
                            Some("MUL") => s.push_str(" INDEX"),
                            _ => {}
                        }
                        s
                    })
                    .collect();
                out.push_str(&rendered.join(", "));
                out.push_str(")\n");
            }
            // Columns are loaded lazily by expanding the tree. Saying "not
            // loaded" beats listing nothing, which reads as "no columns".
            _ => out.push_str(&format!("{kind} `{}` (columns not loaded)\n", t.name)),
        }
    }

    if let Some(routines) = &schema.routines {
        for r in routines {
            let args: Vec<String> = r
                .params
                .iter()
                .map(|p| format!("{} {}", p.name, p.data_type))
                .collect();
            out.push_str(&format!(
                "{} `{}`({})\n",
                r.kind.keyword(),
                r.name,
                args.join(", ")
            ));
        }
    }
    out
}

/// The instructions. Short on purpose — the constraint is the important part.
pub fn system_prompt(server_version: &str, schema: Option<&str>) -> String {
    let mut s = String::from(
        "You are a SQL assistant embedded in a MySQL client. You help the user \
         read, write and understand SQL against the database described below.\n\n\
         You CANNOT run anything. You have no tools and no connection. Every \
         statement you write is placed in the user's editor for them to review \
         and run themselves. Never claim to have run a query, and never report \
         results you have not been given — if you need data to answer, write the \
         query that would produce it and say so.\n\n\
         Put every runnable statement in its own ```sql fenced block, because \
         the app turns those blocks into buttons. Keep prose outside the fences. \
         For anything destructive — DROP, TRUNCATE, DELETE or UPDATE without a \
         WHERE — say plainly what it will do before the block.\n\n",
    );
    s.push_str(&format!("Server: MySQL {server_version}\n\n"));
    match schema {
        Some(schema) if !schema.trim().is_empty() => {
            s.push_str("Schema (names and types only; no row data is shared):\n");
            s.push_str(schema);
        }
        // Better to say so than to let the model invent a plausible schema.
        _ => s.push_str(
            "No schema has been loaded. Ask the user which database and tables \
             they mean rather than guessing at names.",
        ),
    }
    s
}

// ----------------------------------------------------------------- the request

#[derive(Serialize)]
struct Thinking {
    #[serde(rename = "type")]
    kind: &'static str,
    display: &'static str,
}

#[derive(Serialize)]
pub struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: &'a str,
    messages: &'a [ChatMessage],
    thinking: Thinking,
}

/// Body for one streamed completion.
///
/// `thinking.display: "summarized"` is set explicitly: on this model thinking is
/// on by default but its display defaults to omitted, which in a chat window
/// reads as the app having hung.
pub fn request<'a>(model: &'a str, system: &'a str, messages: &'a [ChatMessage]) -> Request<'a> {
    Request {
        model,
        max_tokens: MAX_TOKENS,
        stream: true,
        system,
        messages,
        thinking: Thinking {
            kind: "adaptive",
            display: "summarized",
        },
    }
}

// -------------------------------------------------------------- SSE decoding

/// Incremental Server-Sent Events decoder.
///
/// **The only genuinely tricky part of this module**, and the reason it is a
/// plain struct over `&str` rather than something woven into the HTTP call:
/// chunk boundaries fall wherever the network puts them, so a JSON payload can
/// arrive split across two reads. Buffering until a blank line is what makes
/// that a non-issue, and what makes the whole thing testable without a network.
#[derive(Default)]
pub struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    /// Feed a chunk; get back whatever complete events it completed.
    pub fn push(&mut self, chunk: &str) -> Vec<StreamEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        // Events are separated by a blank line. Anything after the last one is
        // an incomplete event and stays in the buffer for the next chunk.
        while let Some(end) = self.buffer.find("\n\n") {
            let block: String = self.buffer[..end].to_string();
            self.buffer.drain(..end + 2);
            if let Some(e) = decode_block(&block) {
                events.push(e);
            }
        }
        events
    }
}

/// One SSE block into at most one event we care about.
fn decode_block(block: &str) -> Option<StreamEvent> {
    // Only `data:` matters; the `event:` line repeats what the payload says.
    let data = block
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .map(str::trim)?;
    if data.is_empty() || data == "[DONE]" {
        return None;
    }

    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    match v.get("type").and_then(|t| t.as_str())? {
        "content_block_delta" => {
            let delta = v.get("delta")?;
            match delta.get("type").and_then(|t| t.as_str())? {
                "text_delta" => Some(StreamEvent::Text {
                    delta: delta.get("text")?.as_str()?.to_string(),
                }),
                "thinking_delta" => Some(StreamEvent::Thinking {
                    delta: delta.get("thinking")?.as_str()?.to_string(),
                }),
                // input_json_delta and friends: no tools are offered, so
                // anything else is not ours to render.
                _ => None,
            }
        }
        "message_delta" => Some(StreamEvent::Done {
            stop_reason: v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|s| s.as_str())
                .map(str::to_owned),
        }),
        // An error can arrive *mid-stream*, after a 200 and after text has
        // already been shown. Surfacing it as an event rather than discarding
        // the partial answer is the whole reason `Failed` exists.
        "error" => Some(StreamEvent::Failed {
            message: v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("The assistant request failed.")
                .to_string(),
        }),
        _ => None,
    }
}

/// Turn an HTTP failure into something a person can act on.
///
/// The API's own message is included, but the common cases are named first:
/// a mistyped key and an exhausted quota look identical in a raw JSON body.
pub fn http_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(300).collect());

    match status {
        401 => format!("The API key was rejected ({detail}). Check it in Settings."),
        403 => format!("That key is not allowed to use this model ({detail})."),
        429 => format!("Rate limited by the API ({detail}). Try again shortly."),
        500..=599 => format!("The API is having trouble ({status}: {detail})."),
        _ => format!("The assistant request failed ({status}: {detail})."),
    }
}

/// SQL the assistant proposed, so history can say where a statement came from.
///
/// Exact text only. If the user edited it before running, it is theirs — which
/// is the honest answer and needs no fuzzy matching to arrive at.
#[derive(Default)]
pub struct Proposals(HashMap<String, ()>);

impl Proposals {
    pub fn remember(&mut self, sql: &str) {
        self.0.insert(sql.trim().to_string(), ());
    }
    pub fn contains(&self, sql: &str) -> bool {
        self.0.contains_key(sql.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnInfo, TableRef};

    fn text_event(s: &str) -> String {
        format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\
             \"delta\":{{\"type\":\"text_delta\",\"text\":\"{s}\"}}}}\n\n"
        )
    }

    #[test]
    fn decodes_text_deltas_in_order() {
        let mut d = SseDecoder::default();
        let events = d.push(&(text_event("SELECT") + &text_event(" 1")));
        assert_eq!(
            events,
            vec![
                StreamEvent::Text {
                    delta: "SELECT".into()
                },
                StreamEvent::Text { delta: " 1".into() },
            ]
        );
    }

    /// The reason the decoder buffers at all: chunk boundaries land wherever
    /// the network puts them, including the middle of a JSON payload.
    #[test]
    fn an_event_split_across_chunks_survives() {
        let whole = text_event("hello");
        let (a, b) = whole.split_at(whole.len() / 2);

        let mut d = SseDecoder::default();
        assert!(d.push(a).is_empty(), "half an event is not an event");
        assert_eq!(
            d.push(b),
            vec![StreamEvent::Text {
                delta: "hello".into()
            }]
        );
    }

    #[test]
    fn thinking_is_reported_separately_from_the_answer() {
        let mut d = SseDecoder::default();
        let events = d.push(
            "data: {\"type\":\"content_block_delta\",\"delta\":\
             {\"type\":\"thinking_delta\",\"thinking\":\"considering joins\"}}\n\n",
        );
        assert_eq!(
            events,
            vec![StreamEvent::Thinking {
                delta: "considering joins".into()
            }]
        );
    }

    #[test]
    fn a_message_delta_ends_the_turn_and_carries_the_reason() {
        let mut d = SseDecoder::default();
        let events = d.push(
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        );
        assert_eq!(
            events,
            vec![StreamEvent::Done {
                stop_reason: Some("end_turn".into())
            }]
        );
    }

    /// An error can arrive after a 200 and after text has been shown. It must
    /// reach the UI as an event, so what was already written is not thrown away.
    #[test]
    fn a_mid_stream_error_becomes_an_event() {
        let mut d = SseDecoder::default();
        let events = d.push(
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\
             \"message\":\"Overloaded\"}}\n\n",
        );
        assert_eq!(
            events,
            vec![StreamEvent::Failed {
                message: "Overloaded".into()
            }]
        );
    }

    #[test]
    fn unknown_events_and_pings_are_ignored_rather_than_breaking_the_stream() {
        let mut d = SseDecoder::default();
        let events = d.push(
            "event: ping\ndata: {\"type\":\"ping\"}\n\n\
             data: {\"type\":\"content_block_start\",\"index\":0}\n\n",
        );
        assert!(events.is_empty());
        // And the stream still works afterwards.
        assert_eq!(d.push(&text_event("ok")).len(), 1);
    }

    #[test]
    fn a_malformed_payload_does_not_take_the_stream_down() {
        let mut d = SseDecoder::default();
        assert!(d.push("data: {not json\n\n").is_empty());
        assert_eq!(d.push(&text_event("still here")).len(), 1);
    }

    // ------------------------------------------------------------- prompting

    fn schema_fixture() -> DbSchema {
        let mut columns = HashMap::new();
        columns.insert(
            "users".to_string(),
            vec![
                ColumnInfo {
                    name: "id".into(),
                    data_type: "int".into(),
                    nullable: false,
                    key: Some("PRI".into()),
                },
                ColumnInfo {
                    name: "email".into(),
                    data_type: "varchar(190)".into(),
                    nullable: false,
                    key: Some("UNI".into()),
                },
            ],
        );
        DbSchema {
            tables: Some(vec![
                TableRef {
                    name: "users".into(),
                    kind: "BASE TABLE".into(),
                },
                TableRef {
                    name: "user_totals".into(),
                    kind: "VIEW".into(),
                },
            ]),
            columns,
            routines: None,
        }
    }

    #[test]
    fn the_schema_renders_names_types_and_keys() {
        let rendered = render_schema("poc", &schema_fixture());
        assert!(rendered.contains("TABLE `users`"), "{rendered}");
        assert!(rendered.contains("id int NOT NULL PK"), "{rendered}");
        assert!(
            rendered.contains("email varchar(190) NOT NULL UNIQUE"),
            "{rendered}"
        );
        assert!(rendered.contains("VIEW `user_totals`"), "{rendered}");
    }

    /// Columns load lazily. A table whose columns are not cached must say so
    /// rather than appear to have none, which would invite invented column names.
    #[test]
    fn a_table_with_no_cached_columns_says_so() {
        let rendered = render_schema("poc", &schema_fixture());
        assert!(
            rendered.contains("`user_totals` (columns not loaded)"),
            "{rendered}"
        );
    }

    /// The whole feature rests on this instruction.
    #[test]
    fn the_prompt_forbids_claiming_to_have_run_anything() {
        let p = system_prompt("8.4.0", Some("TABLE `users` (id int)"));
        assert!(p.contains("CANNOT run"));
        assert!(p.contains("no tools"));
        assert!(p.contains("```sql"));
        assert!(p.contains("8.4.0"));
    }

    #[test]
    fn with_no_schema_the_prompt_says_to_ask_rather_than_guess() {
        let p = system_prompt("8.4.0", None);
        assert!(p.contains("Ask the user"), "{p}");
        assert!(
            !p.contains("Schema ("),
            "no schema section when there is no schema"
        );
    }

    /// No row data is ever put in the prompt. Asserted on the rendered text,
    /// because this is the promise the UI makes on the feature's behalf.
    #[test]
    fn the_prompt_carries_no_row_data() {
        let rendered = render_schema("poc", &schema_fixture());
        assert!(
            !rendered.contains("@"),
            "an email address reached the prompt"
        );
    }

    // ------------------------------------------------------------- the request

    #[test]
    fn the_request_streams_and_asks_for_visible_thinking() {
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }];
        let body = serde_json::to_value(request(MODEL, "sys", &messages)).unwrap();
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["stream"], true);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "summarized");
        // budget_tokens is rejected outright on this model.
        assert!(body["thinking"].get("budget_tokens").is_none());
        assert_eq!(body["messages"][0]["role"], "user");
    }

    // ---------------------------------------------------------------- errors

    #[test]
    fn a_rejected_key_says_which_key_and_where_to_fix_it() {
        let msg = http_error(401, r#"{"error":{"message":"invalid x-api-key"}}"#);
        assert!(msg.contains("API key"), "{msg}");
        assert!(msg.contains("Settings"), "{msg}");
    }

    #[test]
    fn a_non_json_error_body_still_produces_something_readable() {
        let msg = http_error(502, "<html>Bad Gateway</html>");
        assert!(msg.contains("502"), "{msg}");
        assert!(msg.contains("Bad Gateway"), "{msg}");
    }

    // ------------------------------------------------------------ provenance

    #[test]
    fn a_proposal_is_matched_exactly_and_edits_belong_to_the_user() {
        let mut p = Proposals::default();
        p.remember("SELECT * FROM users");
        assert!(
            p.contains("  SELECT * FROM users  "),
            "whitespace is not an edit"
        );
        assert!(
            !p.contains("SELECT * FROM users WHERE id = 1"),
            "an edited statement is the user's, not the assistant's"
        );
    }
}
