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
    /// What the prompt could carry of the schema, **for the person**.
    ///
    /// The prompt has always told the *model* that columns were listed for only
    /// some tables. The one who can do something about it — ask a narrower
    /// question, name the tables, open them in the tree — was told nothing, and
    /// in the Stage 30 field report that is where the whole failure went
    /// unexplained: the assistant said it was guessing and the person had no
    /// way to see why. Sent once, before the answer. Stage 30 §4.6.
    Context {
        /// Tables described with their columns.
        described: usize,
        /// Tables sent by name alone because the budget ran out, most relevant
        /// first. Empty when everything fitted, which is the common case.
        elided: Vec<String>,
    },
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

/// How many characters of *columns* one request may carry.
///
/// **Measured** — `tests/measure_prompt.rs`, a 400-table × 12-column database
/// shaped like the one in the Stage 30 field report. A table with its columns
/// costs ~315 characters and ~34 named alone, so the whole schema is ~139,000
/// characters (~35k tokens) and the old limit of 60 tables was ~32,600
/// (~8k). Rendering is free at every size: 1.54ms for all 400.
///
/// 60,000 characters is ~15k tokens, or about 190 average tables — three times
/// what the old limit carried, in a budget priced in the thing that actually
/// costs money. **In characters and not tables** because a count prices a
/// 3-column lookup table the same as a 60-column fact table, and because §3
/// measured characters; `DICTIONARY_BUDGET_CHARS` is in characters for the
/// same reason.
///
/// This is *not* [`crate::schema::TABLE_DETAIL_BUDGET`], which bounds round
/// trips on an engine that describes one table at a time. Its own doc comment
/// always said the prompt limit "lives in `render_schema`'s caller" — and then
/// the caller passed it, so the prompt was sized by a number chosen for
/// something else. Stage 30 §4.2.
pub const SCHEMA_BUDGET_CHARS: usize = 60_000;

/// The same, for a schema that travels **once per session** rather than once
/// per question.
///
/// A kept CLI session is primed with the system prompt and reused only while
/// `primed_with` holds, so its schema is sent once and costs nothing per turn
/// thereafter — measured at 1.85-2.66s a turn against 5.3-11.7s for a fresh
/// process. With no per-request cost to protect there is no reason to ration:
/// 240,000 characters is ~60k tokens, which covers ~760 average tables whole.
/// Stage 30 §4.3.
pub const SCHEMA_BUDGET_SESSION_CHARS: usize = 240_000;

/// A rendered schema, and what did not fit in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Tables carried by name alone because the budget ran out, most relevant
    /// first.
    ///
    /// **So the person can be told, not only the model.** The prompt has always
    /// said "columns are listed for the first n tables" to the model; the one
    /// who can do something about it heard nothing. Stage 30 §4.6.
    pub elided: Vec<String>,
    /// Tables whose columns nothing has loaded. Named, and said to be
    /// unloaded, which is honest — but it is a different problem from the
    /// budget and the two should not be reported as one.
    pub unloaded: usize,
}

// ------------------------------------------------- measuring a real schema
//
// Stage 30 shipped a ranking and a budget fitted to a *synthetic* 400-table
// schema (`tests/measure_prompt.rs`), because that is all there was. The two
// functions below are how the numbers get checked against a real one, and they
// answer different questions:
//
//   * [`reckoning`] — what does this database cost a prompt, and how much of
//     it fits? Read off the cache, so the diagnostics dialog answers it with
//     no query and no API call.
//   * [`ranked_note`] — for *this* question, did the right tables get the
//     budget? Only a real question can answer that, so it goes in the log,
//     one line per question, where `report` already copies it out.

/// What a cached schema would cost a prompt, as one line.
///
/// Three renders, ~2ms each at 400 tables, against the cache alone — the
/// diagnostics dialog must not introspect a production server, which is
/// `describe_cache`'s standing rule.
pub fn reckoning(db: &str, schema: &DbSchema) -> String {
    let tables = schema.tables.as_deref().unwrap_or_default().len();
    if tables == 0 {
        return "nothing to describe\n".to_string();
    }
    // No ranking: these are properties of the schema, not of a question.
    let whole = render_schema(db, schema, &[], usize::MAX, None);
    let request = render_schema(db, schema, &[], SCHEMA_BUDGET_CHARS, None);
    let session = render_schema(db, schema, &[], SCHEMA_BUDGET_SESSION_CHARS, None);
    let fits = |r: &Rendered| tables - r.elided.len() - r.unloaded;
    format!(
        "{} chars to describe whole (~{} tokens); {} of {tables} fit a request, \
         {} fit a kept session{}\n",
        whole.text.chars().count(),
        whole.text.chars().count() / 4,
        fits(&request),
        fits(&session),
        match whole.unloaded {
            0 => String::new(),
            n => format!("; {n} have no columns cached"),
        },
    )
}

/// How many names a log line carries before it stops being readable.
const NAMES_IN_A_LINE: usize = 12;

fn listed(names: &[String], n: usize) -> String {
    let shown: Vec<&str> = names.iter().take(n).map(String::as_str).collect();
    if names.len() > n {
        format!("{}, +{} more", shown.join(" "), names.len() - n)
    } else if shown.is_empty() {
        "none".to_string()
    } else {
        shown.join(" ")
    }
}

/// One question's ranking, as a sentence for the log.
///
/// **`landed` is the question's vocabulary as the schema recognised it**, never
/// the question — see [`crate::relevance::recognised`] for why that is both the
/// safer half and the more useful one. `missed` is how many of its words the
/// schema had no name for, as a number, because those are exactly the words
/// that could be anything.
pub fn ranked_note(
    db: &str,
    landed: &[String],
    missed: usize,
    order: &[String],
    out: &Rendered,
    took: std::time::Duration,
) -> String {
    let tables = order.len();
    let described = tables - out.elided.len() - out.unloaded;
    let where_it_is = if db.is_empty() { "schema" } else { db };
    format!(
        "{where_it_is}: {described} of {tables} tables described, {} by name, \
         {} with no columns cached; {} chars; ranked in {}ms; \
         matched [{}] and {missed} word(s) it does not have; first [{}]; cut at [{}]",
        out.elided.len(),
        out.unloaded,
        out.text.chars().count(),
        took.as_millis(),
        listed(landed, 20),
        listed(order, NAMES_IN_A_LINE),
        listed(&out.elided, 5),
    )
}

/// One table, decided but not yet written out.
struct Entry {
    kind: &'static str,
    name: String,
    note: Option<String>,
    join: Option<String>,
    /// `Some` when this table's columns fit: each one rendered, with whatever
    /// the dictionary says about it. `None` means named alone.
    cols: Option<Vec<(String, Option<String>)>>,
    /// Whether anything has loaded its columns at all, which is a different
    /// answer from "they did not fit".
    loaded: bool,
}

/// `name type NOT NULL PK`, the form the prompt has always used.
fn render_column(c: &crate::schema::ColumnInfo) -> String {
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
}

/// Render a database's cached schema as compact text for the system prompt.
///
/// Only names and types — the shape, never the contents — plus whatever the
/// user's own dictionary says about them. Tables whose columns have not been
/// expanded yet appear by name alone, which is honest: it tells the model the
/// table exists without inventing columns for it.
///
/// # Two orders, on purpose
///
/// `order` is the relevance ranking (see [`crate::relevance`]): names most
/// worth describing first. **The budget is spent down that order, and the
/// result is printed alphabetically.** Printing in relevance order would make
/// the block read as though its sequence meant something, and would reshuffle
/// the whole schema whenever the question changed; a schema should read like a
/// schema. Relevance decides *who gets columns*, nothing else.
///
/// A name in `order` that this database does not have is ignored, and a table
/// missing from `order` is considered after every ranked one — so an empty
/// `order` reproduces the alphabetical behaviour this had before Stage 30, and
/// a ranker that forgets a table costs that table its columns rather than its
/// existence.
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
///
/// **A note and a `join` survive the column budget.** They used not to: a
/// table past the old limit took a branch that never charged for them, so the
/// 340 tables sent by name alone lost every note they had — including the one
/// the Stage 30 report turned on, which said that `enrolled_party.status_id`
/// points at `party_status` and not at the similarly named `statuses`. They
/// are short, they are the reason somebody wrote them, and they are what lets
/// a model pick the right table out of a list of names. Stage 30 §2.2.
pub fn render_schema(
    db: &str,
    schema: &DbSchema,
    order: &[String],
    budget_chars: usize,
    dictionary: Option<&crate::dictionary::Dictionary>,
) -> Rendered {
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
        return Rendered {
            text: out,
            ..Default::default()
        };
    }

    // Spent on notes, against `DICTIONARY_BUDGET_CHARS`. Walked in relevance
    // order and charged per table before its columns' — so a dictionary that
    // runs out of budget loses the detail on the tables nobody asked about,
    // and keeps the sentence saying what the relevant ones *are*.
    let mut spent = 0usize;
    let mut dropped = 0usize;
    let mut afford = |text: Option<&str>, spent: &mut usize| -> Option<String> {
        let text = text.map(str::trim).filter(|t| !t.is_empty())?;
        let want = text.chars().count();
        if *spent + want > DICTIONARY_BUDGET_CHARS {
            dropped += 1;
            return None;
        }
        *spent += want;
        Some(text.to_string())
    };

    // Relevance first, then anything the ranking did not mention.
    let mut queue: Vec<&crate::schema::TableRef> = Vec::with_capacity(tables.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for name in order {
        if let Some(t) = tables.iter().find(|t| t.name == *name) {
            if seen.insert(t.name.as_str()) {
                queue.push(t);
            }
        }
    }
    for t in tables {
        if seen.insert(t.name.as_str()) {
            queue.push(t);
        }
    }

    let mut entries: HashMap<&str, Entry> = HashMap::with_capacity(queue.len());
    let mut elided: Vec<String> = Vec::new();
    let mut unloaded = 0usize;
    let mut used = 0usize;

    for t in queue {
        let kind = if t.kind.to_ascii_uppercase().contains("VIEW") {
            "VIEW"
        } else {
            "TABLE"
        };
        let said = dictionary.and_then(|d| d.table(db, &t.name));
        // Charged in the order they matter: what the table *is*, then how it
        // joins, then — only if its columns fit — its columns'.
        let note = afford(said.as_ref().and_then(|s| s.note.as_deref()), &mut spent);
        let join = afford(said.as_ref().and_then(|s| s.join.as_deref()), &mut spent);

        let cached = schema.columns.get(&t.name).filter(|c| !c.is_empty());
        let (cols, loaded) = match cached {
            None => {
                unloaded += 1;
                (None, false)
            }
            Some(cols) => {
                let rendered: Vec<String> = cols.iter().map(render_column).collect();
                // **Priced on the unannotated form**, which is the stable
                // measure of how big a table is. Notes have a budget of their
                // own and are accounted there; pricing them here would make
                // whether a table fits depend on how much somebody wrote
                // about it, and a well-documented table is the last one to
                // drop.
                let cost = rendered
                    .iter()
                    .map(|c| c.chars().count() + 2)
                    .sum::<usize>()
                    + t.name.chars().count()
                    + 10;
                if used + cost > budget_chars {
                    elided.push(t.name.clone());
                    (None, true)
                } else {
                    used += cost;
                    let with_notes = rendered
                        .into_iter()
                        .zip(cols.iter())
                        .map(|(text, c)| {
                            let said = said
                                .as_ref()
                                .and_then(|s| s.columns.get(&c.name.to_lowercase()))
                                .map(String::as_str);
                            (text, afford(said, &mut spent))
                        })
                        .collect();
                    (Some(with_notes), true)
                }
            }
        };

        entries.insert(
            t.name.as_str(),
            Entry {
                kind,
                name: t.name.clone(),
                note,
                join,
                cols,
                loaded,
            },
        );
    }

    // Printed in the order the database reported them, which is alphabetical.
    for t in tables {
        let Some(e) = entries.get(t.name.as_str()) else {
            continue;
        };
        let annotated = e.note.is_some()
            || e.join.is_some()
            || e.cols
                .as_ref()
                .is_some_and(|c| c.iter().any(|(_, n)| n.is_some()));

        match &e.cols {
            // Named alone: the budget ran out, or nothing has loaded them.
            None => {
                let tail = if e.loaded {
                    ""
                } else {
                    " (columns not loaded)"
                };
                out.push_str(&format!("{} `{}`{tail}", e.kind, e.name));
                if let Some(note) = &e.note {
                    out.push_str(&format!(" -- {note}"));
                }
                out.push('\n');
                if let Some(join) = &e.join {
                    out.push_str(&format!("  -- joins: {join}\n"));
                }
            }
            Some(cols) if !annotated => {
                // **Byte for byte what this printed before the dictionary
                // existed.** Nobody without one pays anything for it.
                out.push_str(&format!("{} `{}` (", e.kind, e.name));
                out.push_str(
                    &cols
                        .iter()
                        .map(|(c, _)| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push_str(")\n");
            }
            Some(cols) => {
                out.push_str(&format!("{} `{}`", e.kind, e.name));
                if let Some(note) = &e.note {
                    out.push_str(&format!(" -- {note}"));
                }
                out.push('\n');
                if let Some(join) = &e.join {
                    out.push_str(&format!("  -- joins: {join}\n"));
                }
                for (i, (col, note)) in cols.iter().enumerate() {
                    let open = if i == 0 { "  (" } else { "   " };
                    let close = if i + 1 == cols.len() { ")" } else { "," };
                    match note {
                        Some(note) => out.push_str(&format!("{open}{col}{close} -- {note}\n")),
                        None => out.push_str(&format!("{open}{col}{close}\n")),
                    }
                }
            }
        }
    }

    if !elided.is_empty() {
        let detailed = tables.len() - elided.len() - unloaded;
        out.push_str(&format!(
            "(Columns are listed for the {detailed} tables most likely to be relevant, to keep \
             this short; the {} tables named without them do have columns. Ask the user for a \
             table's columns rather than guessing them.)\n",
            elided.len()
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
    Rendered {
        text: out,
        elided,
        unloaded,
    }
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
        let out = schema_text("", &schema, None);
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

    /// The rendered text with the whole budget and no ranking — the shape every
    /// test below was written for, before Stage 30 gave it an order and a
    /// budget in characters. The two tests that are *about* the budget call it
    /// directly.
    fn schema_text(
        db: &str,
        schema: &DbSchema,
        dictionary: Option<&crate::dictionary::Dictionary>,
    ) -> String {
        render_schema(db, schema, &[], SCHEMA_BUDGET_CHARS, dictionary).text
    }

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
        let rendered = schema_text("poc", &schema_fixture(), None);
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
        let rendered = schema_text("poc", &schema_fixture(), None);
        assert!(
            rendered.contains("`user_totals` (columns not loaded)"),
            "{rendered}"
        );
    }

    /// The cache can hold every column of a large database; the prompt must not.
    #[test]
    fn past_the_limit_tables_are_named_and_the_prompt_says_why() {
        let rendered = render_schema("poc", &schema_fixture(), &[], 0, None).text;
        assert!(!rendered.contains("id int"), "{rendered}");
        assert!(rendered.contains("TABLE `users`\n"), "{rendered}");
        assert!(rendered.contains("do have columns"), "{rendered}");

        let full = schema_text("poc", &schema_fixture(), None);
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
        let rendered = schema_text("poc", &schema_fixture(), None);
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
        let before = schema_text("poc", &schema_fixture(), None);

        let empty = dict("version = 1");
        assert_eq!(schema_text("poc", &schema_fixture(), Some(&empty)), before);

        let elsewhere = dict(
            r#"
            [tables.orders]
            note = "nothing to do with this schema"
            columns.status = "also nothing"
            "#,
        );
        assert_eq!(
            schema_text("poc", &schema_fixture(), Some(&elsewhere)),
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
        let out = schema_text("poc", &schema_fixture(), Some(&d));

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
        let out = schema_text("poc", &schema_fixture(), Some(&d));
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
        let out = schema_text("poc", &schema, Some(&d));
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

        let out = schema_text("poc", &schema, Some(&dict(&toml)));
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

    // ------------------------------- Stage 30: measuring against a real one

    /// What the diagnostics dialog prints per cached database, so the cost of
    /// a real schema can be read off without asking a question and paying for
    /// it.
    #[test]
    fn the_reckoning_says_what_fits_and_what_does_not() {
        let line = reckoning("poc", &four_tables());
        assert!(line.contains("chars to describe whole"), "{line}");
        assert!(line.contains("tokens"), "{line}");
        // Four small tables fit everything.
        assert!(line.contains("4 of 4 fit a request"), "{line}");
        assert!(line.contains("4 fit a kept session"), "{line}");
        // One line, so a report of twelve databases stays readable.
        assert_eq!(line.lines().count(), 1, "{line}");
    }

    /// The case it is for: more schema than a request can carry. A request
    /// describes fewer than a kept session, and both are said, because the two
    /// paths really do send different prompts.
    #[test]
    fn the_reckoning_tells_a_request_apart_from_a_session() {
        let mut columns = HashMap::new();
        let mut tables = Vec::new();
        for i in 0..900 {
            let name = format!("ledger_{i:04}");
            tables.push(TableRef {
                name: name.clone(),
                kind: "BASE TABLE".into(),
            });
            columns.insert(
                name,
                (0..12)
                    .map(|c| ColumnInfo {
                        name: format!("some_column_{c}"),
                        data_type: "decimal(12,2)".into(),
                        nullable: true,
                        key: None,
                    })
                    .collect(),
            );
        }
        let schema = DbSchema {
            tables: Some(tables),
            columns,
            routines: None,
        };
        let line = reckoning("big", &schema);
        let n = |after: &str| -> usize {
            let at = line.find(after).expect(after);
            line[..at]
                .rsplit(|c: char| !c.is_ascii_digit())
                .find(|p| !p.is_empty())
                .unwrap()
                .parse()
                .unwrap()
        };
        let request = n(" of 900 fit a request");
        let session = n(" fit a kept session");
        assert!(request < 900, "a request must not fit all 900: {line}");
        assert!(
            session > request,
            "a kept session pays once and must fit more: {line}"
        );
    }

    #[test]
    fn a_database_with_no_tables_reckons_nothing() {
        let empty = DbSchema::default();
        assert_eq!(reckoning("poc", &empty), "nothing to describe\n");
    }

    /// The per-question line. Its job is to be pasted into an issue, so what
    /// matters is that every number needed to judge the ranking is in it.
    #[test]
    fn the_log_line_carries_what_is_needed_to_judge_the_ranking() {
        let order: Vec<String> = ["delta", "charlie", "bravo", "alpha"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = render_schema("poc", &four_tables(), &order, one_table_budget(), None);
        let line = ranked_note(
            "poc",
            &["reserve".to_string(), "balance".to_string()],
            3,
            &order,
            &out,
            std::time::Duration::from_millis(4),
        );
        assert!(
            line.contains("poc: 1 of 4 tables described, 3 by name"),
            "{line}"
        );
        assert!(line.contains("ranked in 4ms"), "{line}");
        assert!(line.contains("matched [reserve balance]"), "{line}");
        assert!(line.contains("3 word(s) it does not have"), "{line}");
        assert!(line.contains("first [delta charlie bravo alpha]"), "{line}");
        assert!(line.contains("cut at [charlie bravo alpha]"), "{line}");
        // One line: the logbook is read down the page.
        assert_eq!(line.lines().count(), 1, "{line}");
    }

    /// A 400-table server must not put 400 names in one log line.
    #[test]
    fn the_log_line_stays_readable_on_a_big_schema() {
        let order: Vec<String> = (0..400).map(|i| format!("ledger_{i:04}")).collect();
        let out = Rendered {
            text: String::new(),
            elided: order.clone(),
            unloaded: 0,
        };
        let line = ranked_note("big", &[], 0, &order, &out, std::time::Duration::ZERO);
        assert!(line.contains("+388 more"), "{line}");
        assert!(line.contains("+395 more"), "{line}");
        assert!(line.len() < 400, "a log line this long is not read: {line}");
    }

    /// An engine with no namespaces describes itself under the empty one, and
    /// "`: 3 of 4`" reads as a line whose first field went missing.
    #[test]
    fn a_nameless_namespace_is_called_schema_in_the_log() {
        let out = render_schema("", &four_tables(), &[], 1 << 20, None);
        let line = ranked_note("", &[], 0, &[], &out, std::time::Duration::ZERO);
        assert!(line.starts_with("schema:"), "{line}");
    }

    /// The scrubber is the second line of defence and it must not have to be
    /// the first. An ordinary line has to survive it, or every measurement is
    /// replaced by "an entry was dropped".
    #[test]
    fn an_ordinary_log_line_is_not_mistaken_for_a_secret() {
        let order: Vec<String> = ["alpha", "bravo"].iter().map(|s| s.to_string()).collect();
        let out = render_schema("poc", &four_tables(), &order, 1 << 20, None);
        let line = ranked_note(
            "poc",
            &["reserve".to_string(), "balance".to_string()],
            2,
            &order,
            &out,
            std::time::Duration::from_millis(1),
        );
        assert!(
            !crate::logbook::looks_like_a_secret(&line),
            "the scrubber would drop every measurement: {line}"
        );
    }

    // ------------------------- Stage 30: the budget, and the order it spends

    /// Four tables with columns of a known width, so a budget can be set to
    /// admit a chosen number of them.
    fn four_tables() -> DbSchema {
        let mut columns = HashMap::new();
        let mut tables = Vec::new();
        for name in ["alpha", "bravo", "charlie", "delta"] {
            tables.push(TableRef {
                name: name.into(),
                kind: "BASE TABLE".into(),
            });
            columns.insert(
                name.to_string(),
                vec![
                    ColumnInfo {
                        name: "id".into(),
                        data_type: "int".into(),
                        nullable: false,
                        key: Some("PRI".into()),
                    },
                    ColumnInfo {
                        name: "reserve_balance".into(),
                        data_type: "decimal(12,2)".into(),
                        nullable: true,
                        key: None,
                    },
                ],
            );
        }
        DbSchema {
            tables: Some(tables),
            columns,
            routines: None,
        }
    }

    /// The smallest budget that describes exactly one table of `four_tables`.
    ///
    /// **Asked rather than written down.** A number here would be a second
    /// copy of the cost formula, free to be right when it was written and
    /// wrong after the fixture grew a column — and these tests would then be
    /// about whatever the budget happened to admit.
    fn one_table_budget() -> usize {
        (1..1000)
            .find(|b| {
                render_schema("poc", &four_tables(), &[], *b, None)
                    .elided
                    .len()
                    == 3
            })
            .expect("no budget describes exactly one of four equal tables")
    }

    /// The budget is spent down the ranking, not down the alphabet.
    #[test]
    fn the_order_decides_who_gets_columns() {
        let schema = four_tables();
        let one = render_schema("poc", &schema, &[], 1 << 20, None)
            .text
            .matches("reserve_balance")
            .count();
        assert_eq!(one, 4, "the whole budget must describe all four");

        let out = render_schema(
            "poc",
            &schema,
            &["delta".to_string()],
            one_table_budget(),
            None,
        );
        assert!(
            out.text
                .contains("TABLE `delta` (id int NOT NULL PK, reserve_balance decimal(12,2))"),
            "the ranked table did not get the budget: {}",
            out.text
        );
        assert_eq!(out.elided, vec!["alpha", "bravo", "charlie"]);
    }

    /// Spent in relevance order, **printed alphabetically.** A schema whose
    /// sequence moved with the question would read as though the sequence meant
    /// something, and would reshuffle wholesale between two questions.
    #[test]
    fn the_block_is_printed_in_the_databases_own_order() {
        let out = render_schema(
            "poc",
            &four_tables(),
            &["delta".to_string(), "charlie".to_string()],
            1 << 20,
            None,
        );
        let at = |name: &str| out.text.find(name).unwrap();
        assert!(at("alpha") < at("bravo"));
        assert!(at("bravo") < at("charlie"));
        assert!(at("charlie") < at("delta"));
    }

    /// **M4, and the heart of the field report.** A table past the budget used
    /// to take a branch that never charged for its note, so the 340 tables
    /// sent by name alone lost every note they had — including the one that
    /// said `enrolled_party.status_id` points at `party_status` rather than
    /// at the similarly named `statuses`. A name with no note is a name; a
    /// name with one is a reason to pick it.
    #[test]
    fn a_note_and_a_join_survive_the_column_budget() {
        let d = dict(
            "[tables.alpha]\n\
             note = \"one row per enrolment\"\n\
             join = \"alpha.status_id = bravo.id\"\n",
        );
        // Budget enough for exactly one table, given to `delta`.
        let out = render_schema(
            "poc",
            &four_tables(),
            &["delta".to_string()],
            one_table_budget(),
            Some(&d),
        );
        assert!(
            out.elided.contains(&"alpha".to_string()),
            "{:?}",
            out.elided
        );
        assert!(
            !out.text.contains("reserve_balance decimal(12,2))\n   (id"),
            "alpha must not have got its columns: {}",
            out.text
        );
        assert!(
            out.text.contains("TABLE `alpha` -- one row per enrolment"),
            "the note did not survive the budget: {}",
            out.text
        );
        assert!(
            out.text.contains("-- joins: alpha.status_id = bravo.id"),
            "the join did not survive the budget: {}",
            out.text
        );
    }

    /// The other branch that dropped one: a table nobody has loaded kept its
    /// note and has never rendered its `join` at all.
    #[test]
    fn an_unloaded_table_carries_its_join_too() {
        let d = dict(
            "[tables.user_totals]\n\
             note = \"a view over users\"\n\
             join = \"user_totals.user_id = users.id\"\n",
        );
        let out = schema_text("poc", &schema_fixture(), Some(&d));
        assert!(
            out.contains("`user_totals` (columns not loaded) -- a view over users"),
            "{out}"
        );
        assert!(
            out.contains("-- joins: user_totals.user_id = users.id"),
            "{out}"
        );
    }

    /// Named alone is not the same answer as not loaded, and the prompt must
    /// not say the one when it means the other — a model told "columns not
    /// loaded" asks the user to open a tree; one told the budget ran out asks
    /// a narrower question.
    #[test]
    fn a_table_over_budget_is_not_called_unloaded() {
        let out = render_schema("poc", &four_tables(), &[], 0, None);
        assert!(!out.text.contains("columns not loaded"), "{}", out.text);
        assert_eq!(out.unloaded, 0);
        assert_eq!(out.elided.len(), 4);
        assert!(out.text.contains("do have columns"), "{}", out.text);
    }

    /// What the chat tells the person is counted here, so the sentence and the
    /// number cannot disagree.
    #[test]
    fn the_elided_tables_are_reported_most_relevant_first() {
        let out = render_schema(
            "poc",
            &four_tables(),
            &[
                "delta".to_string(),
                "charlie".to_string(),
                "bravo".to_string(),
            ],
            one_table_budget(),
            None,
        );
        assert_eq!(out.elided, vec!["charlie", "bravo", "alpha"]);
    }

    /// Nothing is said about notes when there were none to leave out.
    #[test]
    fn a_dictionary_within_budget_says_nothing_about_budgets() {
        let d = dict("[tables.users]\nnote = \"short\"");
        let out = schema_text("poc", &schema_fixture(), Some(&d));
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
