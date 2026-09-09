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

pub const API_VERSION: &str = "2023-06-01";
/// Streaming, so a long answer cannot hit a request timeout.
pub const MAX_TOKENS: u32 = 16_000;

pub const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
/// Anthropic's most capable model. Writing correct SQL against an unfamiliar
/// schema is the kind of task where model quality shows.
pub const ANTHROPIC_MODEL: &str = "claude-opus-5";

/// Which wire format to speak.
///
/// **Two, not five.** Ollama, LM Studio, llama.cpp's server, vLLM, OpenRouter,
/// Groq and Azure all expose the OpenAI Chat Completions shape, so "OpenAI
/// compatible plus a base URL" covers every one of them. A third adapter would
/// need a provider that speaks neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Anthropic,
    /// Anything speaking `/chat/completions`, including everything local.
    OpenAiCompatible,
}

impl Provider {
    /// Where the key lives. **One account per provider**, so switching away and
    /// back does not lose the other key — and so a local setup with no key at
    /// all cannot shadow a stored cloud one.
    pub fn key_id(self) -> &'static str {
        match self {
            Self::Anthropic => "assistant:anthropic",
            Self::OpenAiCompatible => "assistant:openai",
        }
    }

    /// The completions endpoint under a base URL, whose trailing slash is not
    /// the user's problem to get right.
    pub fn endpoint(self, base: &str) -> String {
        let base = base.trim().trim_end_matches('/');
        match self {
            Self::Anthropic => format!("{base}/v1/messages"),
            Self::OpenAiCompatible => format!("{base}/chat/completions"),
        }
    }

    /// Does a missing key stop us? A local model has none and needs none.
    pub fn requires_key(self, base: &str) -> bool {
        match self {
            Self::Anthropic => true,
            Self::OpenAiCompatible => !is_local(base),
        }
    }
}

/// Is this base URL a machine-local server?
///
/// Used for two things that are both about honesty rather than security: not
/// demanding an API key a local model does not want, and telling the user their
/// schema is not leaving the machine.
pub fn is_local(base: &str) -> bool {
    let b = base.trim().to_ascii_lowercase();
    let host = b
        .split("://")
        .nth(1)
        .unwrap_or(&b)
        .split('/')
        .next()
        .unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    // A bracketed IPv6 literal cannot be split on ':' — `[::1]:8080` would
    // yield "[". Strip the brackets first, then the port.
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") || name.ends_with(".localhost")
}

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
///
/// `caps` decides the dialect and what the model is told it may write. Telling
/// a read-only engine's user to run an `UPDATE` is a confident, useless answer,
/// and the model cannot know the difference unless it is told.
pub fn system_prompt(
    server_version: &str,
    schema: Option<&str>,
    caps: Option<&crate::engine::Capabilities>,
) -> String {
    let engine = caps.map(|c| c.engine.as_str()).unwrap_or("MySQL");
    let mut s = format!(
        "You are a SQL assistant embedded in a {engine} client. You help the user \
         read, write and understand SQL against the database described below.\n\n"
    );
    s.push_str(
        "You CANNOT run anything. You have no tools and no connection. Every \
         statement you write is placed in the user's editor for them to review \
         and run themselves. Never claim to have run a query, and never report \
         results you have not been given — if you need data to answer, write the \
         query that would produce it and say so.\n\n\
         Put every runnable statement in its own ```sql fenced block, because \
         the app turns those blocks into buttons. Keep prose outside the fences. \
         For anything destructive — DROP, TRUNCATE, DELETE or UPDATE without a \
         WHERE — say plainly what it will do before the block.\n\n",
    );

    if let Some(caps) = caps {
        if !caps.writes {
            s.push_str(
                "**This engine is read-only through this interface.** It accepts SELECT, \
                 SHOW and DESCRIBE only — no INSERT, UPDATE, DELETE or DDL, and no \
                 transactions. Never offer one; if the user asks for a change, say that \
                 it cannot be made from here.\n\n",
            );
        }
        if !caps.routines {
            s.push_str("This engine has no stored procedures or functions.\n\n");
        }
        s.push_str(&format!(
            "The user's SQL dialect is {}'s. Its namespaces are called {}s.\n\n",
            caps.engine, caps.namespace_label
        ));
    }
    s.push_str(&format!("Server: {engine} {server_version}\n\n"));
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

/// The body for one streamed completion.
///
/// Built as a `Value` rather than a struct per provider: the two shapes differ
/// in more places than they share, and a struct with half its fields
/// `skip_serializing_if` would obscure which provider gets what.
pub fn request(
    provider: Provider,
    model: &str,
    system: &str,
    messages: &[ChatMessage],
) -> serde_json::Value {
    let turns: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();

    match provider {
        // `thinking.display: "summarized"` is set explicitly: on this model
        // thinking is on by default but its display defaults to omitted, which
        // in a chat window is indistinguishable from the app having hung.
        // `budget_tokens` is deliberately absent — it is rejected outright.
        Provider::Anthropic => serde_json::json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "stream": true,
            "system": system,
            "messages": turns,
            "thinking": { "type": "adaptive", "display": "summarized" },
        }),
        // The system prompt is a message rather than a field, and nothing
        // Anthropic-specific is sent — an unknown parameter is a 400 on some
        // servers and silently ignored on others, and neither is worth risking
        // for a field a local model would not honour anyway.
        Provider::OpenAiCompatible => {
            let mut all = vec![serde_json::json!({ "role": "system", "content": system })];
            all.extend(turns);
            serde_json::json!({
                "model": model,
                "stream": true,
                "messages": all,
            })
        }
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
pub struct SseDecoder {
    provider: Provider,
    buffer: String,
}

impl SseDecoder {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            buffer: String::new(),
        }
    }

    /// Feed a chunk; get back whatever complete events it completed.
    ///
    /// **The framing is the same for both providers** — `data:` lines, events
    /// separated by a blank line — so only the payload decoding forks. That is
    /// the whole reason a second provider was cheap.
    pub fn push(&mut self, chunk: &str) -> Vec<StreamEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        // Anything after the last blank line is an incomplete event and stays
        // in the buffer for the next chunk.
        while let Some(end) = self.buffer.find("\n\n") {
            let block: String = self.buffer[..end].to_string();
            self.buffer.drain(..end + 2);
            let decoded = match self.provider {
                Provider::Anthropic => decode_block(&block),
                Provider::OpenAiCompatible => decode_openai_block(&block),
            };
            if let Some(e) = decoded {
                events.push(e);
            }
        }
        events
    }
}

/// Read the `data:` payload out of one SSE block, if it has a usable one.
fn payload(block: &str) -> Option<serde_json::Value> {
    let data = block
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .map(str::trim)?;
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

/// One OpenAI-style block.
///
/// `choices[0].delta.content` is the answer. `reasoning_content` is what
/// several reasoning models (and Ollama's OpenAI endpoint) put their thinking
/// in; it is read best-effort, because the field is a convention rather than
/// part of the spec, and its absence costs nothing.
fn decode_openai_block(block: &str) -> Option<StreamEvent> {
    let v = payload(block)?;

    // Errors arrive as a plain object rather than inside `choices`.
    if let Some(message) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Some(StreamEvent::Failed {
            message: message.to_string(),
        });
    }

    let choice = v.get("choices")?.get(0)?;
    if let Some(delta) = choice.get("delta") {
        for key in ["reasoning_content", "reasoning"] {
            if let Some(t) = delta.get(key).and_then(|t| t.as_str()) {
                if !t.is_empty() {
                    return Some(StreamEvent::Thinking {
                        delta: t.to_string(),
                    });
                }
            }
        }
        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                return Some(StreamEvent::Text {
                    delta: text.to_string(),
                });
            }
        }
    }

    // A finish reason ends the turn. Checked after the delta because the final
    // chunk can carry both.
    match choice.get("finish_reason") {
        Some(r) if !r.is_null() => Some(StreamEvent::Done {
            stop_reason: r.as_str().map(str::to_owned),
        }),
        _ => None,
    }
}

/// One Anthropic SSE block into at most one event we care about.
fn decode_block(block: &str) -> Option<StreamEvent> {
    // Only `data:` matters; the `event:` line repeats what the payload says.
    let v = payload(block)?;
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
        let mut d = SseDecoder::new(Provider::Anthropic);
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

        let mut d = SseDecoder::new(Provider::Anthropic);
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
        let mut d = SseDecoder::new(Provider::Anthropic);
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
        let mut d = SseDecoder::new(Provider::Anthropic);
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
        let mut d = SseDecoder::new(Provider::Anthropic);
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
        let mut d = SseDecoder::new(Provider::Anthropic);
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
        let mut d = SseDecoder::new(Provider::Anthropic);
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
        let p = system_prompt("8.4.0", Some("TABLE `users` (id int)"), None);
        assert!(p.contains("CANNOT run"));
        assert!(p.contains("no tools"));
        assert!(p.contains("```sql"));
        assert!(p.contains("8.4.0"));
    }

    #[test]
    fn with_no_schema_the_prompt_says_to_ask_rather_than_guess() {
        let p = system_prompt("8.4.0", None, None);
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
        let body = request(Provider::Anthropic, ANTHROPIC_MODEL, "sys", &messages);
        assert_eq!(body["model"], ANTHROPIC_MODEL);
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

    // ------------------------------------------------------- other providers

    fn openai(chunk: &str) -> Vec<StreamEvent> {
        SseDecoder::new(Provider::OpenAiCompatible).push(chunk)
    }

    #[test]
    fn openai_style_deltas_decode_to_the_same_events() {
        let events = openai(
            "data: {\"choices\":[{\"delta\":{\"content\":\"SELECT\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\" 1\"}}]}\n\n",
        );
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

    /// `[DONE]` is OpenAI's terminator and is not JSON. Treating it as a
    /// payload would log a parse failure on every single reply.
    #[test]
    fn the_openai_done_sentinel_is_not_an_error() {
        assert!(openai("data: [DONE]\n\n").is_empty());
    }

    #[test]
    fn an_openai_finish_reason_ends_the_turn() {
        let events = openai("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
        assert_eq!(
            events,
            vec![StreamEvent::Done {
                stop_reason: Some("stop".into())
            }]
        );
    }

    /// Several reasoning models — and Ollama's OpenAI endpoint — stream their
    /// thinking in `reasoning_content`. It is a convention, not a spec, so it
    /// is read best-effort and its absence costs nothing.
    #[test]
    fn openai_reasoning_content_is_shown_as_thinking() {
        let events =
            openai("data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hmm\"}}]}\n\n");
        assert_eq!(
            events,
            vec![StreamEvent::Thinking {
                delta: "hmm".into()
            }]
        );
    }

    #[test]
    fn an_openai_error_object_becomes_a_failure() {
        let events = openai("data: {\"error\":{\"message\":\"model not found\"}}\n\n");
        assert_eq!(
            events,
            vec![StreamEvent::Failed {
                message: "model not found".into()
            }]
        );
    }

    /// Empty deltas are how OpenAI streams open and close a turn. Rendering
    /// them would put stray empty text blocks through the UI.
    #[test]
    fn empty_openai_deltas_produce_nothing() {
        assert!(openai("data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n").is_empty());
        assert!(
            openai("data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n").is_empty()
        );
    }

    // ------------------------------------------------------------ endpoints

    #[test]
    fn an_endpoint_is_built_regardless_of_a_trailing_slash() {
        assert_eq!(
            Provider::Anthropic.endpoint("https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            Provider::OpenAiCompatible.endpoint("http://localhost:11434/v1"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    /// Each provider gets its own keychain account, so switching away and back
    /// does not lose the other key.
    #[test]
    fn providers_do_not_share_a_key() {
        assert_ne!(
            Provider::Anthropic.key_id(),
            Provider::OpenAiCompatible.key_id()
        );
    }

    #[test]
    fn a_local_server_is_recognised_and_needs_no_key() {
        for base in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://[::1]:8080/v1",
            "http://LocalHost:11434",
        ] {
            assert!(is_local(base), "{base}");
            assert!(!Provider::OpenAiCompatible.requires_key(base), "{base}");
        }
    }

    /// The check must not be fooled into calling a remote host local — that
    /// would suppress the warning that the schema is leaving the machine.
    #[test]
    fn a_remote_host_is_never_mistaken_for_a_local_one() {
        for base in [
            "https://api.openai.com/v1",
            "https://localhost.example.com/v1",
            "http://evil.com/?x=localhost",
            "http://user@example.com/v1",
        ] {
            assert!(!is_local(base), "{base}");
            assert!(Provider::OpenAiCompatible.requires_key(base), "{base}");
        }
    }

    #[test]
    fn an_openai_request_carries_the_system_prompt_as_a_message() {
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }];
        let body = request(Provider::OpenAiCompatible, "llama3.1", "rules", &messages);
        assert_eq!(body["model"], "llama3.1");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "rules");
        assert_eq!(body["messages"][1]["content"], "hi");
        // Nothing Anthropic-specific: an unknown parameter is a 400 on some
        // servers and silently ignored on others.
        assert!(body.get("thinking").is_none());
        assert!(body.get("system").is_none());
    }

    /// A read-only engine must be told so, or it will confidently write an
    /// `UPDATE` the user cannot run.
    #[test]
    fn a_read_only_engine_is_declared_to_the_model() {
        let caps = crate::elastic::capabilities();
        let p = system_prompt("8.15.0", Some("TABLE `orders` (total double)"), Some(&caps));
        assert!(p.contains("read-only"), "{p}");
        assert!(p.contains("no INSERT, UPDATE, DELETE"), "{p}");
        assert!(p.contains("elasticsearch client"), "{p}");
        assert!(p.contains("catalogs"), "{p}");
        assert!(p.contains("no stored procedures"), "{p}");
    }

    /// MySQL gains no restrictions it did not have.
    #[test]
    fn an_engine_with_writes_gets_no_refusal_text() {
        let caps = crate::engine::Capabilities::mysql();
        let p = system_prompt("8.4.0", None, Some(&caps));
        assert!(!p.contains("read-only"), "{p}");
        assert!(p.contains("mysql client"), "{p}");
    }
}
