//! Driving an agent CLI the person is already signed into.
//!
//! # Why a CLI at all
//!
//! The assistant (Stage 10) needs a model, and until now that meant an API key
//! and a per-token bill. Someone who already pays for Claude Code or Copilot
//! CLI has a model on their PATH and a session already signed in. Stage 20 is
//! about using it.
//!
//! This is not the MCP server (Stage 13), which answers the opposite direction:
//! there, an outside tool asks *us* about the connection. Here the person is in
//! this app, looking at this connection, and asks from here.
//!
//! # Recipes, not a command field
//!
//! The thing being configured is **which of a closed list of CLIs to use** —
//! never a command line. A setting that holds a command is arbitrary code
//! execution with a friendlier label, and this is a database client. Adding a
//! CLI is a commit, which is the point.
//!
//! # The invariant, and why it is checked rather than configured
//!
//! `assistant.rs` gives the model no tools, because the app's oldest rule is
//! that the person at the keyboard presses run, and the way that rule is kept
//! is that there is nothing to erode. A CLI is an *agent*: tools are its
//! default state, so every recipe has to take them away.
//!
//! Measured 2026-10-01, and the reason this module reads the tool list back
//! instead of trusting its own arguments:
//!
//! * Copilot's `--available-tools=` with an **empty** value is silently
//!   ignored. The run went out with 21 tools — `bash`, `create`, `edit` — and
//!   nothing said so. Naming a tool that does not exist does empty it, but
//!   that is a trick, not a contract, and it would break *open*.
//! * Claude's `--disallowedTools "mcp__*"` empties the tool list but **not**
//!   the server list: four claude.ai connectors still loaded. No tools were
//!   exposed, so the rule held — but it held for a different reason than the
//!   flag implied.
//!
//! Both CLIs state what they actually sent. So the recipe reads it back and
//! [`CliEvent::Tools`] carries it to a caller that refuses to continue unless
//! it is empty. An invariant that is asserted survives the vendor changing
//! their mind; one that is configured does not.

use crate::assistant::ChatMessage;

/// A tool name that does not exist, which is how Copilot's allowlist is
/// emptied.
///
/// `--available-tools=` (empty) is ignored; `--available-tools=<unknown>`
/// yields `tool_count: 0`. Deliberately named after this app so that if it
/// ever *does* match something, the collision is obvious in a tool list.
const NO_SUCH_TOOL: &str = "__db_query_no_tools__";

/// Which CLI. A closed list — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Recipe {
    Claude,
    Copilot,
}

/// Which stream shape a recipe's output has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// `claude --output-format stream-json`: one JSON object per line, with
    /// Anthropic-shaped `stream_event` deltas inside.
    ClaudeStreamJson,
    /// `copilot --output-format json`: JSONL of `{"type": "assistant.message_delta", ...}`.
    CopilotJsonl,
}

/// What a line of a CLI's output meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliEvent {
    /// The process is up and talking. Ends the *starting* state in the UI,
    /// which matters because Copilot says nothing for its first ~2.5s.
    Started,
    /// The tool set the CLI actually exposed to the model. **Asserted empty by
    /// the caller** — see the module docs.
    Tools { names: Vec<String> },
    /// Text for the person.
    Text { delta: String },
    /// The model called a tool. Must never happen; the caller aborts.
    ToolUse { name: String },
    /// Not signed in. Its own event because it is the one failure with a
    /// remedy worth printing.
    NotSignedIn,
    /// The turn ended. `error` is set when the CLI reported failure.
    Done { error: Option<String> },
}

impl Recipe {
    /// The binary we look for on PATH.
    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
        }
    }

    /// For Settings and for error messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Copilot => "Copilot CLI",
        }
    }

    pub fn wire(self) -> Wire {
        match self {
            Self::Claude => Wire::ClaudeStreamJson,
            Self::Copilot => Wire::CopilotJsonl,
        }
    }

    /// Arguments for one question.
    ///
    /// `cwd` is an empty scratch directory: both CLIs are coding agents that
    /// read their working directory by default, and neither is being invited
    /// into the person's project.
    ///
    /// `system` is a file holding our system prompt. Claude can replace its
    /// own with it; **Copilot cannot** — it has no such flag, so there the
    /// caller folds our rules into the prompt text instead and this argument
    /// is unused.
    pub fn ask_args(
        self,
        system: &std::path::Path,
        prompt: &str,
        cwd: &std::path::Path,
    ) -> Vec<String> {
        let s = |x: &str| x.to_string();
        match self {
            // `--bare` is deliberately absent. It looks exactly right — it
            // skips hooks, skills, CLAUDE.md and MCP discovery — but measured
            // 2026-10-01 it also breaks authentication: with it, every run
            // returns "Not logged in · Please run /login" and
            // `duration_api_ms: 0`, while `claude auth status` says the
            // account is signed in. So the person's configuration does load,
            // and the tool flags below carry the whole weight. Which is why
            // the `init` line is read back.
            Self::Claude => vec![
                s("-p"),
                prompt.to_string(),
                // No tools, and no MCP tools either.
                s("--tools"),
                s(""),
                s("--disallowedTools"),
                s("mcp__*"),
                // Nothing can answer a permission prompt here, so denial is
                // the honest setting rather than a hang.
                s("--permission-prompts"),
                s("none"),
                s("--max-turns"),
                s("1"),
                // The conversation is ours; it is not also written under
                // ~/.claude.
                s("--no-session-persistence"),
                s("--disable-slash-commands"),
                // Load no MCP servers at all. The flag alone did not empty the
                // server list (four connectors still loaded), so this names an
                // empty set as the only source.
                s("--strict-mcp-config"),
                s("--mcp-config"),
                s("{\"mcpServers\":{}}"),
                s("--system-prompt-file"),
                system.display().to_string(),
                s("--output-format"),
                s("stream-json"),
                s("--include-partial-messages"),
                s("--verbose"),
            ],
            Self::Copilot => vec![
                s("-p"),
                prompt.to_string(),
                // See NO_SUCH_TOOL: the empty value is ignored.
                format!("--available-tools={NO_SUCH_TOOL}"),
                s("--no-custom-instructions"),
                s("--disable-builtin-mcps"),
                s("--no-ask-user"),
                s("--disallow-temp-dir"),
                s("--no-remote"),
                s("--no-remote-export"),
                // Do not swap the binary out from under a recipe measured
                // against this version.
                s("--no-auto-update"),
                // Our prompt carries the person's schema; it does not need to
                // land in ~/.copilot/logs as well.
                s("--log-level"),
                s("none"),
                s("--output-format"),
                s("json"),
                s("-C"),
                cwd.display().to_string(),
            ],
        }
    }

    /// Arguments for holding this CLI open as an ACP session.
    ///
    /// The same hardening as [`Self::ask_args`] — the allowlist that matches
    /// nothing, no custom instructions, no built-in MCP servers, no logging of
    /// the person's schema — minus the one-shot pieces: there is no `-p`, and
    /// the output format is the protocol itself rather than JSONL.
    ///
    /// Only Copilot has this. Claude's long-lived transport is
    /// `--input-format stream-json`, which is a different shape and not yet
    /// built, so Claude still runs one process per message.
    pub fn session_args(self) -> Option<Vec<String>> {
        let s = |x: &str| x.to_string();
        match self {
            Self::Claude => None,
            Self::Copilot => Some(vec![
                s("--acp"),
                format!("--available-tools={NO_SUCH_TOOL}"),
                s("--no-custom-instructions"),
                s("--disable-builtin-mcps"),
                s("--no-ask-user"),
                s("--disallow-temp-dir"),
                s("--no-remote"),
                s("--no-remote-export"),
                s("--no-auto-update"),
                s("--log-level"),
                s("none"),
            ]),
        }
    }

    /// Arguments for a readiness check that **costs nothing** — no model call,
    /// no billable request.
    ///
    /// Both exist, which is what makes warming the integration at startup
    /// honest rather than decorative:
    ///
    /// * Claude: `auth status` prints JSON and exits 0 or 1.
    /// * Copilot: has no such subcommand, and its token lives in the OS
    ///   credential store, which is not ours to read. But `--acp` — Agent
    ///   Client Protocol, a JSON-RPC server mode — answers `session/new` with
    ///   `{"error": {"code": -32000, "message": "Authentication required"}}`
    ///   when signed out and a live session when signed in. Measured
    ///   2026-10-01. The caller drives that handshake; this returns the
    ///   arguments for it.
    pub fn probe_args(self) -> Vec<String> {
        match self {
            Self::Claude => vec!["auth".to_string(), "status".to_string()],
            Self::Copilot => vec!["--acp".to_string()],
        }
    }
}

/// Rebuild the conversation as text for a one-shot invocation.
///
/// Phase 1 spends a process per message, so there is nothing to carry the
/// roles: the turns are labelled in the prompt instead. A long-lived process
/// (`claude --input-format stream-json`, or Copilot over ACP) keeps real roles
/// and is the next step, not this one.
///
/// The last user message is left as itself at the end rather than labelled,
/// because that is the actual question and a model answers a question better
/// than it answers a transcript.
pub fn transcript(messages: &[ChatMessage]) -> String {
    let (last, earlier) = match messages.split_last() {
        Some(pair) => pair,
        None => return String::new(),
    };
    if earlier.is_empty() {
        return last.content.trim().to_string();
    }
    let mut out = String::from("Earlier in this conversation:\n\n");
    for m in earlier {
        let who = if m.role == "assistant" { "You" } else { "User" };
        out.push_str(who);
        out.push_str(": ");
        out.push_str(m.content.trim());
        out.push_str("\n\n");
    }
    out.push_str("User's question now:\n\n");
    out.push_str(last.content.trim());
    out
}

/// Read one line of a CLI's output.
///
/// Returns nothing for the many lines that are neither text nor a fact we act
/// on — a malformed line included. A reply must not die because a vendor added
/// an event type.
pub fn parse_line(wire: Wire, line: &str) -> Vec<CliEvent> {
    let line = line.trim();
    if line.is_empty() {
        return vec![];
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return vec![];
    };
    match wire {
        Wire::ClaudeStreamJson => claude_line(&v),
        Wire::CopilotJsonl => copilot_line(&v),
    }
}

/// Claude's `--output-format stream-json`.
fn claude_line(v: &serde_json::Value) -> Vec<CliEvent> {
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            let names = v["tools"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            vec![CliEvent::Started, CliEvent::Tools { names }]
        }
        // With `--include-partial-messages` the text arrives as Anthropic's own
        // delta events, which is the same shape `assistant.rs` already decodes
        // over SSE — but wrapped one level deeper.
        Some("stream_event") => {
            let ev = &v["event"];
            if ev["type"] == "content_block_delta" {
                let d = &ev["delta"];
                let kind = d["type"].as_str().unwrap_or("");
                if kind == "text_delta" {
                    if let Some(t) = d["text"].as_str() {
                        return vec![CliEvent::Text {
                            delta: t.to_string(),
                        }];
                    }
                }
            }
            if ev["type"] == "content_block_start" && ev["content_block"]["type"] == "tool_use" {
                return vec![CliEvent::ToolUse {
                    name: ev["content_block"]["name"]
                        .as_str()
                        .unwrap_or("?")
                        .to_string(),
                }];
            }
            vec![]
        }
        // The whole message. Ignored for text — the deltas already carried it,
        // and counting both would double every answer — but it is where a
        // signed-out run says so, because that path produces no deltas at all.
        Some("assistant") => {
            let text: String = v["message"]["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if is_signed_out(&text) {
                vec![CliEvent::NotSignedIn]
            } else {
                vec![]
            }
        }
        Some("result") => {
            let failed = v["is_error"].as_bool().unwrap_or(false);
            let error = if failed {
                // `subtype` is "success" even on an error run, so it is not the
                // field to read. The result text is the useful part.
                Some(
                    v["result"]
                        .as_str()
                        .unwrap_or("The CLI reported an error.")
                        .to_string(),
                )
            } else {
                None
            };
            vec![CliEvent::Done { error }]
        }
        _ => vec![],
    }
}

/// Copilot's `--output-format json`.
fn copilot_line(v: &serde_json::Value) -> Vec<CliEvent> {
    match v["type"].as_str() {
        Some("session.info") | Some("session.auto_mode_resolved") => vec![CliEvent::Started],
        Some("assistant.message_delta") => match v["data"]["deltaContent"].as_str() {
            Some(t) => vec![CliEvent::Text {
                delta: t.to_string(),
            }],
            None => vec![],
        },
        // Copilot reports its tool set at the *end* of a turn, inside a usage
        // checkpoint, rather than at the start. So this arrives too late to
        // stop the first answer — which is why `ToolUse` below is the guard
        // that acts during the turn, and this one is what refuses to carry the
        // conversation any further.
        Some("session.usage_checkpoint") => {
            let names = checkpoint_tools(v);
            vec![CliEvent::Tools { names }]
        }
        Some("assistant.tool_call") | Some("tool.start") | Some("tool.invocation") => {
            vec![CliEvent::ToolUse {
                name: v["data"]["name"].as_str().unwrap_or("?").to_string(),
            }]
        }
        Some("result") => {
            let code = v["exitCode"].as_i64().unwrap_or(0);
            vec![CliEvent::Done {
                error: (code != 0)
                    .then(|| format!("{} exited with code {code}.", Recipe::Copilot.label())),
            }]
        }
        _ => vec![],
    }
}

/// The tool names buried in Copilot's usage checkpoint.
///
/// The shape is `promptCacheBreakState[].models.<model>.tools[].name`, which is
/// deep enough that it is walked rather than typed: the path is incidental to
/// us and the vendor owes us no stability in it. `tool_count` sits beside it
/// and agrees; the names are what make a violation legible in a message.
fn checkpoint_tools(v: &serde_json::Value) -> Vec<String> {
    let mut names = Vec::new();
    let states = match v["data"]["promptCacheBreakState"].as_array() {
        Some(a) => a,
        None => return names,
    };
    for state in states {
        let Some(models) = state["models"].as_object() else {
            continue;
        };
        for (_, model) in models {
            if let Some(tools) = model["tools"].as_array() {
                for t in tools {
                    if let Some(n) = t["name"].as_str() {
                        names.push(n.to_string());
                    }
                }
            }
        }
    }
    names
}

/// Does this text mean "not signed in"?
///
/// Matched on the message because neither CLI gives a code for it on the
/// streaming path: Claude answers the question with
/// `Not logged in · Please run /login` and exits 1. Narrow on purpose — a
/// reply that merely *discusses* logging in should not be mistaken for one.
fn is_signed_out(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t.starts_with("not logged in")
        || t.starts_with("no authentication information found")
        || t.starts_with("authentication required")
}

// ----------------------------------------------------------------- running it

use crate::assistant::StreamEvent;
use std::process::Stdio;
use tokio::io::AsyncBufReadExt;

/// A scratch directory and, for Claude, the system-prompt file in it.
///
/// Both CLIs are coding agents that read their working directory by default,
/// so neither is invited into the person's project: each run gets an empty
/// directory of its own and it is removed afterwards.
struct Scratch {
    dir: std::path::PathBuf,
    system: std::path::PathBuf,
}

impl Scratch {
    fn make(system: &str) -> Result<Self, String> {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("db-query-cli-{n:016x}"));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not make a scratch directory: {e}"))?;
        let path = dir.join("system.txt");
        std::fs::write(&path, system).map_err(|e| format!("Could not write the prompt: {e}"))?;
        Ok(Self { dir, system: path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover directory in the system temp is untidy, not
        // broken, and a failed cleanup must not fail an answer.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Stream one reply out of a CLI.
///
/// Mirrors the HTTP path's contract: anything that goes wrong after the first
/// token arrives as a [`StreamEvent::Failed`] rather than a returned error, so
/// half an answer is not thrown away to show a message.
pub async fn run(
    recipe: Recipe,
    system: &str,
    messages: &[ChatMessage],
    cancelled: &crate::session::CliCancel,
    mut emit: impl FnMut(StreamEvent),
) -> Result<(), String> {
    if cancelled.was_asked() {
        return Ok(());
    }
    let scratch = Scratch::make(system)?;

    // Claude replaces its system prompt with ours. Copilot has no such flag —
    // measured 2026-10-01 — so there the rules ride in the prompt instead.
    let prompt = match recipe {
        Recipe::Claude => transcript(messages),
        Recipe::Copilot => format!("{system}\n\n---\n\n{}", transcript(messages)),
    };

    let args = recipe.ask_args(&scratch.system, &prompt, &scratch.dir);

    // The environment is **inherited**, which was not the original plan.
    //
    // `env_clear()` was: it reads as the careful choice. But both CLIs
    // authenticate out of the person's own session — Claude from under `HOME`,
    // Copilot from the OS credential store, which on Linux needs the session
    // D-Bus address — so clearing the environment does not harden the call, it
    // breaks it. What made clearing cheap to give up is that there is nothing
    // of ours to leak: a database password lives in the keychain and is read at
    // the moment it is used, never parked in our environment.
    let mut child = crate::proc::command(recipe.program())
        .args(&args)
        .current_dir(&scratch.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // If this future is dropped — the window closes, the task is abandoned —
        // the CLI does not keep running and billing.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{} is not installed: `{}` was not found on your PATH.",
                recipe.label(),
                recipe.program()
            ),
            _ => format!("Could not start {}: {e}", recipe.program()),
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or("The CLI produced no output stream.")?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let wire = recipe.wire();

    let mut said_anything = false;
    let mut ended = false;
    // Copilot announces itself more than once — `session.info` and
    // `session.auto_mode_resolved` both mean "up". The UI only needs to leave
    // the starting state once.
    let mut announced = false;
    let mut refusal: Option<String> = None;

    // Armed before the first read, so a cancel landing in between is not lost —
    // the same shape `session::cancellable` uses for a query.
    let notified = cancelled.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    loop {
        let line = tokio::select! {
            read = lines.next_line() => match read {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    emit(StreamEvent::Failed { message: format!("The reply was cut off: {e}") });
                    break;
                }
            },
            _ = &mut notified => {
                // Killing is the only way to stop a child, and the only cost is
                // a process that was going to exit anyway.
                let _ = child.kill().await;
                return Ok(());
            }
        };

        for event in parse_line(wire, &line) {
            match event {
                CliEvent::Started if !announced => {
                    announced = true;
                    emit(StreamEvent::Started);
                }
                CliEvent::Started => {}
                CliEvent::Text { delta } => {
                    said_anything = true;
                    emit(StreamEvent::Text { delta });
                }
                // **The invariant.** See the module docs for why this is read
                // back rather than trusted to the flags.
                CliEvent::Tools { names } if !names.is_empty() => {
                    refusal = Some(format!(
                        "{} offered the model {} tool{} ({}). The assistant is not allowed any, \
                         so this answer was stopped. Please report this — the flags that remove \
                         them have changed.",
                        recipe.label(),
                        names.len(),
                        if names.len() == 1 { "" } else { "s" },
                        names.join(", ")
                    ));
                }
                CliEvent::Tools { .. } => {}
                CliEvent::ToolUse { name } => {
                    refusal = Some(format!(
                        "{} tried to use a tool ({name}). The assistant has no tools and runs \
                         nothing, so this answer was stopped.",
                        recipe.label()
                    ));
                }
                CliEvent::NotSignedIn => {
                    refusal = Some(format!(
                        "{} is not signed in. Run `{} {}` in a terminal, then try again.",
                        recipe.label(),
                        recipe.program(),
                        match recipe {
                            Recipe::Claude => "auth login",
                            Recipe::Copilot => "login",
                        }
                    ));
                }
                CliEvent::Done { error } => {
                    ended = true;
                    match error {
                        Some(message) if !said_anything => refusal = Some(message),
                        // It failed *after* writing something. The text on
                        // screen is the better half; say it was cut short
                        // rather than replacing it with the CLI's own summary.
                        Some(_) => emit(StreamEvent::Failed {
                            message: format!("{} stopped early.", recipe.label()),
                        }),
                        None => {}
                    }
                }
            }
        }

        if refusal.is_some() {
            let _ = child.kill().await;
            break;
        }
    }

    if let Some(message) = refusal {
        emit(StreamEvent::Failed { message });
        return Ok(());
    }

    // Nothing at all came back. The reason is usually on stderr, and a bare
    // "it failed" when the CLI said why is a worse message than the CLI's.
    if !said_anything && !ended {
        let mut why = String::new();
        if let Some(mut err) = child.stderr.take() {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await;
            why = String::from_utf8_lossy(&buf).trim().to_string();
        }
        let status = child.wait().await.ok();
        let code = status.and_then(|s| s.code()).unwrap_or(-1);
        let detail = if why.is_empty() {
            format!("exited with code {code} and said nothing")
        } else {
            why.lines().take(4).collect::<Vec<_>>().join(" ")
        };
        emit(StreamEvent::Failed {
            message: format!("{} {detail}.", recipe.label()),
        });
        return Ok(());
    }

    emit(StreamEvent::Done { stop_reason: None });
    let _ = child.wait().await;
    Ok(())
}

/// Ask through a **kept session**, starting one if needed.
///
/// Returns `Ok(false)` when this recipe has no session transport, so the caller
/// falls back to [`run`]. Today that is Claude; see [`Recipe::session_args`].
///
/// A session is reused only while it was primed with the same system prompt.
/// The schema rides in the first turn, so a session opened in one database is
/// answering about that one — comparing the whole prompt catches every reason
/// it could have gone stale without enumerating them.
pub async fn run_session(
    state: &crate::session::AppState,
    recipe: Recipe,
    system: &str,
    messages: &[ChatMessage],
    cancelled: &crate::session::CliCancel,
    mut emit: impl FnMut(StreamEvent),
) -> Result<bool, String> {
    let Some(args) = recipe.session_args() else {
        return Ok(false);
    };

    // The newest question only: the session is holding the rest. On the first
    // turn the rules and the schema go with it, because Copilot has no
    // system-prompt flag and this is the one place they fit.
    let question = messages
        .last()
        .map(|m| m.content.trim().to_string())
        .unwrap_or_default();

    let existing = {
        let agents = state.cli_agents.lock().await;
        agents.get(&recipe).cloned()
    };
    let agent = match existing {
        Some(agent) if agent.primed_with(system) => agent,
        other => {
            // Stale or absent. A stale one is closed rather than left running:
            // it holds a process and a conversation about a schema nobody is
            // looking at any more.
            if let Some(old) = other {
                old.close().await;
            }
            let scratch = Scratch::make("")?;
            let fresh = std::sync::Arc::new(
                crate::acp::Agent::start(recipe.program(), &args, &scratch.dir, system.to_string())
                    .await
                    .map_err(|e| format!("{}: {e}", recipe.label()))?,
            );
            // The scratch directory has to outlive the process that is running
            // in it, so the guard is deliberately leaked here rather than
            // dropped at the end of this block. It is an empty directory in the
            // system temp; the alternative is a CLI whose working directory
            // disappears under it mid-session.
            std::mem::forget(scratch);
            state.cli_agents.lock().await.insert(recipe, fresh.clone());
            fresh
        }
    };

    // Said before the first turn as well as after: starting a session is the
    // slow part, and by here it is already done.
    emit(StreamEvent::Started);

    let first = messages.len() <= 1;
    let text = if first {
        format!("{system}\n\n---\n\n{question}")
    } else {
        question
    };

    let outcome = agent
        .prompt(&text, cancelled, |o| match o {
            crate::acp::Outcome::Text(delta) => emit(StreamEvent::Text { delta }),
            crate::acp::Outcome::Failed(message) => emit(StreamEvent::Failed { message }),
        })
        .await;

    match outcome {
        // Cancelled. The session survives, which is the point of asking it to
        // stop rather than killing it.
        Ok(None) => {}
        Ok(Some(stop_reason)) => emit(StreamEvent::Done {
            stop_reason: Some(stop_reason),
        }),
        Err(message) => {
            // A broken session is not reused: whatever went wrong, the next
            // question should start from something known.
            agent.close().await;
            state.cli_agents.lock().await.remove(&recipe);
            emit(StreamEvent::Failed { message });
        }
    }
    Ok(true)
}

// ------------------------------------------------------------- is it ready?

/// What a readiness probe found. Everything here is knowable **without a model
/// call**, which is what makes warming an integration at startup honest rather
/// than a way to spend someone's quota on a lamp.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub installed: bool,
    pub signed_in: bool,
    /// What to say to the person. Empty when it is simply ready.
    pub detail: String,
}

/// Probe a recipe. Never asks a model anything, never costs a request.
pub async fn probe(recipe: Recipe) -> Status {
    match recipe {
        Recipe::Claude => probe_claude(recipe).await,
        Recipe::Copilot => probe_copilot(recipe).await,
    }
}

fn missing(recipe: Recipe) -> Status {
    Status {
        installed: false,
        signed_in: false,
        detail: format!(
            "`{}` was not found on your PATH. Install {} to use it here.",
            recipe.program(),
            recipe.label()
        ),
    }
}

/// `claude auth status` prints JSON and exits 0 when signed in, 1 when not.
async fn probe_claude(recipe: Recipe) -> Status {
    let out = crate::proc::command(recipe.program())
        .args(recipe.probe_args())
        .stdin(Stdio::null())
        .output()
        .await;
    let out = match out {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return missing(recipe),
        Err(e) => {
            return Status {
                installed: true,
                signed_in: false,
                detail: format!("Could not ask {}: {e}", recipe.label()),
            }
        }
    };
    // The exit code alone would do, but the JSON says it in a field and a field
    // is harder to misread than a convention.
    let signed_in = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .ok()
        .and_then(|v| v["loggedIn"].as_bool())
        .unwrap_or_else(|| out.status.success());
    Status {
        installed: true,
        signed_in,
        detail: if signed_in {
            String::new()
        } else {
            format!(
                "Not signed in. Run `{} auth login` in a terminal.",
                recipe.program()
            )
        },
    }
}

/// Copilot has no `auth status`, and its token lives in the OS credential
/// store, which is not ours to read.
///
/// So this speaks **ACP** — the Agent Client Protocol, its `--acp` mode — over
/// stdin and stdout: `initialize`, then `session/new`. Measured 2026-10-01,
/// signing out produces
/// `{"error":{"code":-32000,"message":"Authentication required"}}` and signing
/// in produces a live session, with no model call either way.
async fn probe_copilot(recipe: Recipe) -> Status {
    let dir = match Scratch::make("") {
        Ok(d) => d,
        Err(detail) => {
            return Status {
                installed: true,
                signed_in: false,
                detail,
            }
        }
    };
    let mut child = match crate::proc::command(recipe.program())
        .args(recipe.probe_args())
        .current_dir(&dir.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return missing(recipe),
        Err(e) => {
            return Status {
                installed: true,
                signed_in: false,
                detail: format!("Could not start {}: {e}", recipe.label()),
            }
        }
    };

    let handshake = format!(
        "{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false}}}}"#,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": { "cwd": dir.dir.display().to_string(), "mcpServers": [] }
        })
    );
    if let Some(mut stdin) = child.stdin.take() {
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stdin, handshake.as_bytes()).await;
        // Dropped, so the CLI sees end-of-input and does not wait for more.
    }

    let answer = match child.stdout.take() {
        Some(stdout) => read_acp_answer(stdout).await,
        None => None,
    };
    let _ = child.kill().await;

    match answer {
        Some(Ok(())) => Status {
            installed: true,
            signed_in: true,
            detail: String::new(),
        },
        Some(Err(message)) => Status {
            installed: true,
            signed_in: false,
            detail: format!("{message} Run `{} login` in a terminal.", recipe.program()),
        },
        // It started and never answered the handshake. Installed, and nothing
        // more can be claimed — which the caller shows as exactly that.
        None => Status {
            installed: true,
            signed_in: false,
            detail: format!(
                "{} did not answer. It is installed; whether it is signed in is unknown.",
                recipe.label()
            ),
        },
    }
}

/// Read ACP lines until the answer to `session/new` (id 1) arrives.
///
/// `Ok(())` is a session; `Err` is the refusal's message. A notification that
/// happens to arrive first — the agent advertises its commands — is not an
/// answer, so the loop keeps reading rather than guessing from the first line.
async fn read_acp_answer(stdout: tokio::process::ChildStdout) -> Option<Result<(), String>> {
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    // Bounded: a CLI that chatters forever must not hold the probe open, and a
    // handshake takes two lines when it works.
    for _ in 0..64 {
        let line = match tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
            .await
        {
            Ok(Ok(Some(line))) => line,
            _ => return None,
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["id"].as_i64() != Some(1) {
            continue;
        }
        if let Some(message) = v["error"]["message"].as_str() {
            return Some(Err(format!("{message}.")));
        }
        if v.get("result").is_some() {
            return Some(Ok(()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fixture below is a real line, captured from the probes on
    // 2026-10-01 and shortened only where a field is irrelevant. A parser
    // tested against invented output is a parser tested against its author.

    #[test]
    fn claude_init_reports_an_empty_tool_set() {
        let line = r#"{"type":"system","subtype":"init","tools":[],"mcp_servers":[],"model":"claude-opus-5-5"}"#;
        assert_eq!(
            parse_line(Wire::ClaudeStreamJson, line),
            vec![CliEvent::Started, CliEvent::Tools { names: vec![] }]
        );
    }

    /// The case the readback exists for. If the flags ever stop working this
    /// is what the app sees, and it must see the names.
    #[test]
    fn claude_init_reports_tools_it_was_told_not_to_have() {
        let line = r#"{"type":"system","subtype":"init","tools":["Bash","Edit"],"mcp_servers":[]}"#;
        let events = parse_line(Wire::ClaudeStreamJson, line);
        assert_eq!(
            events[1],
            CliEvent::Tools {
                names: vec!["Bash".into(), "Edit".into()]
            }
        );
    }

    #[test]
    fn claude_text_arrives_as_deltas() {
        let a = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"1\n2\n3\n4"}}}"#;
        let b = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"\n5\n6"}}}"#;
        assert_eq!(
            parse_line(Wire::ClaudeStreamJson, a),
            vec![CliEvent::Text {
                delta: "1\n2\n3\n4".into()
            }]
        );
        assert_eq!(
            parse_line(Wire::ClaudeStreamJson, b),
            vec![CliEvent::Text {
                delta: "\n5\n6".into()
            }]
        );
    }

    /// The full message repeats what the deltas already said. Counting both
    /// would print every answer twice.
    #[test]
    fn claude_full_message_adds_no_text() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"1\n2\n3"}]}}"#;
        assert_eq!(parse_line(Wire::ClaudeStreamJson, line), vec![]);
    }

    #[test]
    fn claude_says_when_it_is_not_signed_in() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Not logged in · Please run /login"}]}}"#;
        assert_eq!(
            parse_line(Wire::ClaudeStreamJson, line),
            vec![CliEvent::NotSignedIn]
        );
    }

    /// `subtype` is "success" on a failed run, so it is not the field to read.
    #[test]
    fn a_failed_claude_run_is_an_error_despite_its_subtype() {
        let line =
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#;
        assert_eq!(
            parse_line(Wire::ClaudeStreamJson, line),
            vec![CliEvent::Done {
                error: Some("Not logged in".into())
            }]
        );
    }

    #[test]
    fn a_tool_call_is_reported_rather_than_ignored() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Bash"}}}"#;
        assert_eq!(
            parse_line(Wire::ClaudeStreamJson, line),
            vec![CliEvent::ToolUse {
                name: "Bash".into()
            }]
        );
    }

    #[test]
    fn copilot_text_arrives_as_deltas() {
        let line =
            r#"{"type":"assistant.message_delta","data":{"messageId":"e6","deltaContent":"OK"}}"#;
        assert_eq!(
            parse_line(Wire::CopilotJsonl, line),
            vec![CliEvent::Text { delta: "OK".into() }]
        );
    }

    /// Shortened from the real checkpoint, keeping the nesting exactly.
    #[test]
    fn copilot_checkpoint_yields_the_tool_names_it_sent() {
        let line = r#"{"type":"session.usage_checkpoint","data":{"promptCacheBreakState":[{"conversation":"main","models":{"mai-code-1.1-flash":{"tool_count":2,"tools":[{"name":"bash"},{"name":"edit"}]}}}]}}"#;
        assert_eq!(
            parse_line(Wire::CopilotJsonl, line),
            vec![CliEvent::Tools {
                names: vec!["bash".into(), "edit".into()]
            }]
        );
    }

    #[test]
    fn copilot_checkpoint_with_no_tools_is_an_empty_list() {
        let line = r#"{"type":"session.usage_checkpoint","data":{"promptCacheBreakState":[{"models":{"m":{"tool_count":0,"tools":[]}}}]}}"#;
        assert_eq!(
            parse_line(Wire::CopilotJsonl, line),
            vec![CliEvent::Tools { names: vec![] }]
        );
    }

    #[test]
    fn copilot_result_carries_its_exit_code() {
        let ok = r#"{"type":"result","sessionId":"48","exitCode":0}"#;
        assert_eq!(
            parse_line(Wire::CopilotJsonl, ok),
            vec![CliEvent::Done { error: None }]
        );
        let bad = r#"{"type":"result","sessionId":"48","exitCode":1}"#;
        match &parse_line(Wire::CopilotJsonl, bad)[0] {
            CliEvent::Done { error: Some(m) } => assert!(m.contains("code 1")),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// A vendor adding an event type must not end a reply.
    #[test]
    fn unknown_and_malformed_lines_are_skipped() {
        for line in [
            r#"{"type":"session.tools_updated","data":{}}"#,
            r#"{"type":"model.call_start","data":{}}"#,
            "not json at all",
            "{",
            "",
        ] {
            assert_eq!(parse_line(Wire::CopilotJsonl, line), vec![], "{line}");
            assert_eq!(parse_line(Wire::ClaudeStreamJson, line), vec![], "{line}");
        }
    }

    // ---------------------------------------------------------- the arguments

    /// These flags *are* the invariant's first line of defence, so they are
    /// pinned. A refactor that drops one is a refactor that offers the model a
    /// shell.
    #[test]
    fn the_claude_recipe_takes_every_tool_away() {
        let args = Recipe::Claude.ask_args(
            std::path::Path::new("/tmp/sys.txt"),
            "hello",
            std::path::Path::new("/tmp/empty"),
        );
        let joined = args.join(" ");
        assert!(joined.contains("--tools  "), "{joined}");
        assert!(joined.contains("--disallowedTools mcp__*"));
        assert!(joined.contains("--strict-mcp-config"));
        assert!(joined.contains("--permission-prompts none"));
        assert!(joined.contains("--no-session-persistence"));
        assert!(joined.contains("--system-prompt-file /tmp/sys.txt"));
        // And `--bare` is absent on purpose: it breaks authentication.
        assert!(!joined.contains("--bare"), "{joined}");
    }

    #[test]
    fn the_copilot_recipe_empties_its_allowlist_with_a_name_that_matches_nothing() {
        let args = Recipe::Copilot.ask_args(
            std::path::Path::new("/tmp/sys.txt"),
            "hello",
            std::path::Path::new("/tmp/empty"),
        );
        let joined = args.join(" ");
        // Not `--available-tools=`, which is silently ignored.
        assert!(
            joined.contains(&format!("--available-tools={NO_SUCH_TOOL}")),
            "{joined}"
        );
        assert!(!joined.contains("--available-tools= "), "{joined}");
        assert!(joined.contains("--no-custom-instructions"));
        assert!(joined.contains("--disable-builtin-mcps"));
        assert!(joined.contains("--no-ask-user"));
        assert!(joined.contains("--log-level none"));
        assert!(joined.contains("-C /tmp/empty"));
        // Nothing that would hand it the person's machine.
        assert!(!joined.contains("--allow-all"), "{joined}");
        assert!(!joined.contains("--yolo"), "{joined}");
    }

    #[test]
    fn a_probe_never_asks_a_model_anything() {
        assert_eq!(Recipe::Claude.probe_args(), vec!["auth", "status"]);
        assert_eq!(Recipe::Copilot.probe_args(), vec!["--acp"]);
    }

    // ---------------------------------------------------------- the transcript

    #[test]
    fn one_question_is_sent_as_itself() {
        let msgs = vec![ChatMessage {
            role: "user".into(),
            content: "  which tables hold orders?  ".into(),
        }];
        assert_eq!(transcript(&msgs), "which tables hold orders?");
    }

    #[test]
    fn a_conversation_keeps_its_turns_and_ends_on_the_question() {
        let msgs = vec![
            ChatMessage {
                role: "user".into(),
                content: "list the tables".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "orders, users".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "and their columns?".into(),
            },
        ];
        let t = transcript(&msgs);
        assert!(t.starts_with("Earlier in this conversation:"));
        assert!(t.contains("User: list the tables"));
        assert!(t.contains("You: orders, users"));
        assert!(t.trim_end().ends_with("and their columns?"));
    }

    #[test]
    fn no_messages_is_an_empty_prompt_rather_than_a_panic() {
        assert_eq!(transcript(&[]), "");
    }
}
