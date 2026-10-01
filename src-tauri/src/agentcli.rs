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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    pub fn ask_args(self, system: &std::path::Path, prompt: &str, cwd: &std::path::Path) -> Vec<String> {
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
                error: (code != 0).then(|| format!("{} exited with code {code}.", Recipe::Copilot.label())),
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
            vec![
                CliEvent::Started,
                CliEvent::Tools { names: vec![] }
            ]
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
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"1\n2\n3"}]}}"#;
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
        let line = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#;
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
            vec![CliEvent::ToolUse { name: "Bash".into() }]
        );
    }

    #[test]
    fn copilot_text_arrives_as_deltas() {
        let line = r#"{"type":"assistant.message_delta","data":{"messageId":"e6","deltaContent":"OK"}}"#;
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
        assert!(joined.contains(&format!("--available-tools={NO_SUCH_TOOL}")), "{joined}");
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
