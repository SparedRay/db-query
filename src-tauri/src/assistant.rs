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
    /// The provider is up and about to answer. **Only a CLI sends this**, and
    /// it earns its place: Copilot CLI says nothing at all for its first ~2.5
    /// seconds while it starts and refreshes its token, and an empty bubble
    /// for two and a half seconds reads as broken. The HTTP providers have
    /// nothing to report here, so they do not.
    Started,
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

/// How many characters of dictionary prose may be added to one prompt.
///
/// **Measured, not chosen to look reasonable** (Stage 25 §2): the schema block
/// for 60 tables of 10 columns is ~24,600 characters, a sentence per table adds
/// ~21%, and a phrase on *every* column adds ~131% — doubling the prompt in
/// every request. Real annotation is sparse, so the budget is generous for the
/// shape people actually write and protective against a dictionary that
/// annotates everything: 8,000 characters is 60 table notes at ~90 characters
/// plus ~30 column notes. Past that a prompt is paying more for prose than for
/// the schema it describes.
pub const DICTIONARY_BUDGET_CHARS: usize = 8_000;

/// Render a database's cached schema as compact text for the system prompt.
///
/// Only names and types — the shape, never the contents — plus whatever the
/// user's own dictionary says about them. Tables whose columns have not been
/// expanded yet appear by name alone, which is honest: it tells the model the
/// table exists without inventing columns for it.
///
/// **At most `detail_limit` tables are rendered with their columns.** The
/// cache can now hold every column of a large database, and 385 tables of them
/// is tens of thousands of tokens in every request. Past the limit a table is
/// named alone and the prompt says why, so the model asks rather than assumes
/// the table has no columns.
///
/// # Where a note goes
///
/// **Beside the thing it describes**, as a `--` comment, never as a glossary
/// section after the schema (Stage 25 §3.2): a note printed next to its own
/// column cannot be misattributed, where a separate list of definitions asks
/// the model to join two lists by name — and costs the same tokens.
///
/// A table nobody wrote about renders **exactly as it did before this
/// existed**, on one line. The multi-line form is what paying for notes buys,
/// so the cost stays proportional to what was written.
pub fn render_schema(
    db: &str,
    schema: &DbSchema,
    detail_limit: usize,
    dictionary: Option<&crate::dictionary::Dictionary>,
) -> String {
    let mut out = String::new();
    // An engine without namespaces — a local Elasticsearch cluster has no
    // catalogs — describes itself under the empty one, and "Database ``" reads
    // as a database whose name we failed to find rather than as its absence.
    if db.is_empty() {
        out.push_str("Schema\n");
    } else {
        out.push_str(&format!("Database `{db}`\n"));
    }

    let tables = schema.tables.as_deref().unwrap_or_default();
    if tables.is_empty() {
        out.push_str("(no tables have been loaded yet)\n");
        return out;
    }

    let mut detailed = 0;
    let mut elided = 0;
    // Spent on notes, against `DICTIONARY_BUDGET_CHARS`. Tables are walked in
    // the order they are rendered, and each one's own note is paid for before
    // its columns' — so a dictionary that runs out of budget loses the detail
    // first and keeps the sentence that says what the table *is*.
    let mut spent = 0usize;
    let mut dropped = 0usize;
    for t in tables {
        let kind = if t.kind.to_ascii_uppercase().contains("VIEW") {
            "VIEW"
        } else {
            "TABLE"
        };
        // What the dictionary says about this one, if anything.
        let said = dictionary.and_then(|d| d.table(db, &t.name));
        // **Charged before anything is rendered**, because the budget decides
        // the *shape* of the entry and not merely its text: a table whose note
        // the budget dropped must fall back to the one-line form, or losing
        // the note would still cost the newlines and the indentation that
        // having one buys. Its own test caught that.
        let mut afford = |text: Option<&str>| -> Option<String> {
            let text = text.map(str::trim).filter(|t| !t.is_empty())?;
            let want = text.chars().count();
            if spent + want > DICTIONARY_BUDGET_CHARS {
                dropped += 1;
                return None;
            }
            spent += want;
            Some(text.to_string())
        };

        match schema.columns.get(&t.name) {
            Some(cols) if !cols.is_empty() && detailed >= detail_limit => {
                elided += 1;
                out.push_str(&format!("{kind} `{}`\n", t.name));
            }
            Some(cols) if !cols.is_empty() => {
                detailed += 1;

                // Charged in the order they matter: what the table *is*,
                // then how it joins, then its columns. A dictionary that runs
                // out of budget therefore loses detail before it loses the
                // sentence that explains the table.
                let table_note = afford(said.as_ref().and_then(|s| s.note.as_deref()));
                let join = afford(said.as_ref().and_then(|s| s.join.as_deref()));
                let notes: Vec<Option<String>> = cols
                    .iter()
                    .map(|c| {
                        let said = said
                            .as_ref()
                            .and_then(|s| s.columns.get(&c.name.to_lowercase()))
                            .map(String::as_str);
                        afford(said)
                    })
                    .collect();

                let annotated =
                    table_note.is_some() || join.is_some() || notes.iter().any(Option::is_some);

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

                if !annotated {
                    // **Byte for byte what this printed before the dictionary
                    // existed.** Nobody without one pays anything for it.
                    out.push_str(&format!("{kind} `{}` (", t.name));
                    out.push_str(&rendered.join(", "));
                    out.push_str(")\n");
                    continue;
                }

                out.push_str(&format!("{kind} `{}`", t.name));
                if let Some(note) = &table_note {
                    out.push_str(&format!(" -- {note}"));
                }
                out.push('\n');
                if let Some(join) = &join {
                    out.push_str(&format!("  -- joins: {join}\n"));
                }
                for (i, col) in rendered.iter().enumerate() {
                    let open = if i == 0 { "  (" } else { "   " };
                    let close = if i + 1 == rendered.len() { ")" } else { "," };
                    match &notes[i] {
                        Some(note) => out.push_str(&format!("{open}{col}{close} -- {note}\n")),
                        None => out.push_str(&format!("{open}{col}{close}\n")),
                    }
                }
            }
            // Columns are loaded lazily by expanding the tree. Saying "not
            // loaded" beats listing nothing, which reads as "no columns".
            _ => {
                let note = afford(said.as_ref().and_then(|s| s.note.as_deref()));
                match note {
                    Some(note) => out.push_str(&format!(
                        "{kind} `{}` (columns not loaded) -- {note}\n",
                        t.name
                    )),
                    None => out.push_str(&format!("{kind} `{}` (columns not loaded)\n", t.name)),
                }
            }
        }
    }
    if elided > 0 {
        out.push_str(&format!(
            "(Columns are listed for the first {detailed} tables only, to keep this short; \
             the {elided} tables named without them do have columns. Ask the user for a \
             table's columns rather than guessing them.)\n"
        ));
    }
    if dropped > 0 {
        // Said rather than silently truncated, for the same reason the elision
        // above is said: a model told nothing assumes nothing was there.
        out.push_str(&format!(
            "({dropped} of the user's notes were left out to keep this short. Ask them \
             about a table rather than assuming it has no note.)\n"
        ));
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
    // Whether the schema block carries the user's own notes, which changes
    // what the `--` comments in it *are*.
    dictionary: bool,
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
        // Two reasons, two sentences — the same distinction the refusal makes.
        // Telling the model "this engine is read-only" about a MySQL
        // connection somebody ticked a box on is false, and it is the kind of
        // false that comes back as a confidently wrong explanation.
        if caps.read_only {
            s.push_str(
                "**This connection is marked read-only by the user.** It accepts SELECT, \
                 SHOW and DESCRIBE only — no INSERT, UPDATE, DELETE or DDL. Never offer \
                 one; if the user asks for a change, say that this connection refuses \
                 writes and that the setting is theirs to change.\n\n",
            );
        } else if !caps.writes {
            s.push_str(
                "**This engine is read-only through this interface.** It accepts SELECT, \
                 SHOW and DESCRIBE only — no INSERT, UPDATE, DELETE or DDL. Never offer \
                 one; if the user asks for a change, say that it cannot be made from \
                 here.\n\n",
            );
        }
        // Its own clause, not a rider on the one above. A read-only connection
        // to MySQL still has transactions; only the engine's own limit takes
        // them away, and saying otherwise taught the model something untrue.
        if !caps.transactions {
            s.push_str("This engine has no transactions or session statements.\n\n");
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
            if dictionary {
                // **Said after the schema, not before it.** The model has just
                // read the comments; this is what tells it whose they are and
                // that they outrank a guess from a column's name — which is
                // the whole point of somebody writing them down.
                s.push_str(
                    "\nThe `--` comments above are the user's own notes on this database, \
                     from a dictionary they maintain alongside it. Treat them as \
                     authoritative: where a note and a name disagree, the note is right. \
                     A table or column with no note is not described anywhere — ask \
                     rather than inferring meaning from its name.\n",
                );
            }
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
    /// An engine with no namespaces must not be described as a nameless one.
    #[test]
    fn an_empty_namespace_is_not_rendered_as_an_empty_name() {
        let schema = DbSchema {
            tables: Some(vec![crate::schema::TableRef {
                name: "orders".into(),
                kind: "TABLE".into(),
            }]),
            columns: Default::default(),
            routines: None,
        };
        let out = render_schema("", &schema, 60, None);
        assert!(!out.contains("``"), "{out}");
        assert!(out.starts_with("Schema"), "{out}");
    }

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
        let rendered = render_schema("poc", &schema_fixture(), 60, None);
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
        let rendered = render_schema("poc", &schema_fixture(), 60, None);
        assert!(
            rendered.contains("`user_totals` (columns not loaded)"),
            "{rendered}"
        );
    }

    /// The cache can hold every column of a large database; the prompt must not.
    #[test]
    fn past_the_limit_tables_are_named_and_the_prompt_says_why() {
        let rendered = render_schema("poc", &schema_fixture(), 0, None);
        assert!(!rendered.contains("id int"), "{rendered}");
        assert!(rendered.contains("TABLE `users`\n"), "{rendered}");
        assert!(rendered.contains("do have columns"), "{rendered}");

        let full = render_schema("poc", &schema_fixture(), 60, None);
        assert!(!full.contains("do have columns"), "{full}");
    }

    /// The whole feature rests on this instruction.
    #[test]
    fn the_prompt_forbids_claiming_to_have_run_anything() {
        let p = system_prompt("8.4.0", Some("TABLE `users` (id int)"), None, false);
        assert!(p.contains("CANNOT run"));
        assert!(p.contains("no tools"));
        assert!(p.contains("```sql"));
        assert!(p.contains("8.4.0"));
    }

    #[test]
    fn with_no_schema_the_prompt_says_to_ask_rather_than_guess() {
        let p = system_prompt("8.4.0", None, None, false);
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
        let rendered = render_schema("poc", &schema_fixture(), 60, None);
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
        let p = system_prompt(
            "8.15.0",
            Some("TABLE `orders` (total double)"),
            Some(&caps),
            false,
        );
        assert!(p.contains("read-only"), "{p}");
        assert!(p.contains("no INSERT, UPDATE, DELETE"), "{p}");
        assert!(p.contains("elasticsearch client"), "{p}");
        assert!(p.contains("catalogs"), "{p}");
        assert!(p.contains("no stored procedures"), "{p}");
    }

    /// A read-only **connection** is a different sentence from a read-only
    /// **engine**, and must not borrow the engine's other limits either.
    #[test]
    fn a_read_only_connection_is_described_as_one() {
        let mut profile = crate::session::ConnProfile {
            id: "p".into(),
            name: "prod".into(),
            colour: "#ef4444".into(),
            host: "h".into(),
            port: 3306,
            user: "u".into(),
            database: None,
            allow_invalid_certs: false,
            kind: Default::default(),
            url: String::new(),
            auth: Default::default(),
            no_password: false,
            read_only: true,
            dictionary: None,
        };
        let caps = crate::engine::Capabilities::mysql().for_profile(&profile);
        let p = system_prompt("8.4.0", None, Some(&caps), false);
        assert!(p.contains("connection is marked read-only"), "{p}");
        assert!(!p.contains("This engine is read-only"), "{p}");
        // MySQL keeps its transactions; only the engine's own limit removes
        // them, and this used to be asserted as a rider on "read-only".
        assert!(!p.contains("no transactions"), "{p}");

        profile.read_only = false;
        let plain = system_prompt(
            "8.4.0",
            None,
            Some(&crate::engine::Capabilities::mysql().for_profile(&profile)),
            false,
        );
        assert!(!plain.contains("read-only"), "{plain}");
    }

    // ------------------------------------------ Stage 25: the dictionary

    fn dict(toml: &str) -> crate::dictionary::Dictionary {
        crate::dictionary::parse(toml)
            .expect("the fixture dictionary parses")
            .dictionary
    }

    /// **The promise that nobody pays for a feature they do not use.**
    ///
    /// A schema with no dictionary, an empty dictionary, and a dictionary about
    /// some other database's tables must all render the same bytes — the ones
    /// this function produced before any of this existed.
    #[test]
    fn a_table_nobody_described_renders_exactly_as_before() {
        let before = render_schema("poc", &schema_fixture(), 60, None);

        let empty = dict("version = 1");
        assert_eq!(
            render_schema("poc", &schema_fixture(), 60, Some(&empty)),
            before
        );

        let elsewhere = dict(
            r#"
            [tables.orders]
            note = "nothing to do with this schema"
            columns.status = "also nothing"
            "#,
        );
        assert_eq!(
            render_schema("poc", &schema_fixture(), 60, Some(&elsewhere)),
            before,
            "a dictionary about other tables must not change this one's bytes"
        );
    }

    /// §3.2: beside the thing it describes. The note on a column is on that
    /// column's own line, and the table's is on the table's.
    #[test]
    fn a_note_is_rendered_beside_what_it_describes() {
        let d = dict(
            r#"
            [tables.users]
            note = "One row per sign-in identity."
            join = "users.id = user_totals.user_id"
            columns.email = "Lower-cased by the app, never by the database."
            "#,
        );
        let out = render_schema("poc", &schema_fixture(), 60, Some(&d));

        assert!(
            out.contains("TABLE `users` -- One row per sign-in identity.\n"),
            "{out}"
        );
        assert!(
            out.contains("  -- joins: users.id = user_totals.user_id\n"),
            "{out}"
        );
        // On the column's line, and only that column's.
        assert!(
            out.contains(
                "   email varchar(190) NOT NULL UNIQUE) -- Lower-cased by the app, never by the database.\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("  (id int NOT NULL PK,\n"),
            "no note, no comment: {out}"
        );
        // The view nobody described keeps its one-line form.
        assert!(
            out.contains("VIEW `user_totals` (columns not loaded)\n"),
            "{out}"
        );
    }

    /// A table whose columns have not been loaded still gets its own note —
    /// that is the one the model needs most, having no columns to reason from.
    #[test]
    fn an_unloaded_table_still_carries_its_note() {
        let d = dict(
            r#"
            [tables.user_totals]
            note = "Per-user rollup, rebuilt nightly."
            "#,
        );
        let out = render_schema("poc", &schema_fixture(), 60, Some(&d));
        assert!(
            out.contains(
                "VIEW `user_totals` (columns not loaded) -- Per-user rollup, rebuilt nightly.\n"
            ),
            "{out}"
        );
    }

    /// §3.5 again, at the other end: a dictionary is keyed lower-case and the
    /// server may report `Users`.
    #[test]
    fn the_case_the_server_reports_does_not_hide_a_note() {
        let mut schema = schema_fixture();
        schema.tables = Some(vec![TableRef {
            name: "Users".into(),
            kind: "BASE TABLE".into(),
        }]);
        schema.columns = HashMap::from([(
            "Users".to_string(),
            vec![ColumnInfo {
                name: "Email".into(),
                data_type: "varchar(190)".into(),
                nullable: false,
                key: None,
            }],
        )]);

        let d = dict(
            r#"
            [tables.users]
            note = "found anyway"
            columns.email = "and so is this"
            "#,
        );
        let out = render_schema("poc", &schema, 60, Some(&d));
        assert!(out.contains("TABLE `Users` -- found anyway"), "{out}");
        assert!(out.contains("-- and so is this"), "{out}");
    }

    /// The budget bites, and **says that it bit**. A model told nothing
    /// assumes nothing was there — the same reason the column elision above
    /// announces itself.
    #[test]
    fn the_note_budget_is_spent_and_what_it_cost_is_said() {
        // One note per table, each a twentieth of the budget, over enough
        // tables to exhaust it twice.
        let long = "x".repeat(DICTIONARY_BUDGET_CHARS / 20);
        let mut toml = String::from("version = 1\n");
        let mut tables = Vec::new();
        let mut columns = HashMap::new();
        for i in 0..40 {
            let name = format!("t{i}");
            toml.push_str(&format!("\n[tables.{name}]\nnote = \"{long}\"\n"));
            tables.push(TableRef {
                name: name.clone(),
                kind: "BASE TABLE".into(),
            });
            columns.insert(
                name,
                vec![ColumnInfo {
                    name: "id".into(),
                    data_type: "int".into(),
                    nullable: false,
                    key: None,
                }],
            );
        }
        let schema = DbSchema {
            tables: Some(tables),
            columns,
            routines: None,
        };

        let out = render_schema("poc", &schema, 60, Some(&dict(&toml)));
        let spent: usize = out.matches(&long).count();
        assert_eq!(spent, 20, "exactly the budget, not one note more: {spent}");
        assert!(
            out.contains("20 of the user's notes were left out"),
            "and it must say so: {out}"
        );
        // The tables past the budget are still listed, with their columns —
        // losing the note must not lose the schema.
        assert!(out.contains("TABLE `t39` (id int NOT NULL)"), "{out}");
    }

    /// Nothing is said about notes when there were none to leave out.
    #[test]
    fn a_dictionary_within_budget_says_nothing_about_budgets() {
        let d = dict("[tables.users]\nnote = \"short\"");
        let out = render_schema("poc", &schema_fixture(), 60, Some(&d));
        assert!(!out.contains("left out"), "{out}");
    }

    /// D4's other half: the model is told **whose** the comments are, and only
    /// when there are some. Without this the `--` text is just more schema.
    #[test]
    fn the_prompt_says_whose_the_notes_are_only_when_there_are_notes() {
        let with = system_prompt(
            "8.4.0",
            Some("TABLE `users` (id int) -- people"),
            None,
            true,
        );
        assert!(with.contains("the user's own notes"), "{with}");
        assert!(with.contains("authoritative"), "{with}");
        assert!(
            with.contains("ask rather than inferring"),
            "and that a silent table is not a described one: {with}"
        );

        let without = system_prompt("8.4.0", Some("TABLE `users` (id int)"), None, false);
        assert!(!without.contains("the user's own notes"), "{without}");

        // And never when there is no schema at all to comment on.
        let nothing = system_prompt("8.4.0", None, None, true);
        assert!(!nothing.contains("the user's own notes"), "{nothing}");
    }

    /// MySQL gains no restrictions it did not have.
    #[test]
    fn an_engine_with_writes_gets_no_refusal_text() {
        let caps = crate::engine::Capabilities::mysql();
        let p = system_prompt("8.4.0", None, Some(&caps), false);
        assert!(!p.contains("read-only"), "{p}");
        assert!(p.contains("mysql client"), "{p}");
    }
}
