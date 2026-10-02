//! A long-lived agent over the **Agent Client Protocol**.
//!
//! # Why this exists
//!
//! Phase 1 spent a process per message, and measured 2026-10-01 that cost
//! 5.3–11.7 seconds *every time*. The same CLI held open as an ACP session
//! costs 2.4 s once and then 1.85–2.66 s a turn. The saving is not the headline
//! though: a session also **holds the conversation**, so the schema goes over
//! once instead of with every question, and a cancel can be a polite
//! `session/cancel` rather than a kill — which means stopping a reply no longer
//! throws the session away.
//!
//! # What ACP is, as far as we use it
//!
//! JSON-RPC 2.0, one object per line, over the child's stdin and stdout:
//!
//! 1. `initialize` — we advertise **no filesystem capability**, so the agent
//!    may not ask us to read or write anything.
//! 2. `session/new` — gives a session id. This is also the readiness probe:
//!    signed out, it answers `Authentication required` without calling a model.
//! 3. `session/prompt` — the question. Text comes back as `session/update`
//!    notifications carrying `agent_message_chunk`, and the request's own
//!    response ends the turn with a `stopReason`.
//! 4. `session/cancel` — a notification, not a request. The turn then ends with
//!    `stopReason: "cancelled"` and the session survives.
//!
//! # The two things measuring taught us
//!
//! **The agent talks to the user inside the answer.** A turn's first chunks
//! were `Info: Disabled tools: bash, create, edit, …` and `Info: Unknown tool
//! name in the tool allowlist: "…"` — the same channel as the reply, so a naive
//! renderer opens every answer with two lines of our own plumbing.
//! [`is_plumbing`] filters exactly those forms and a test pins their shape.
//!
//! **The tool set cannot be read back here.** The one-shot path gets a count
//! and the names as data; ACP's `usage_update` carries tokens only. What it does
//! give is that `Disabled tools:` line, which *names what was removed*. So the
//! rule is enforced by three weaker things instead of one strong one — the
//! flags, a refusal on any `tool_call`, and [`guards_hold`] checking that line
//! names the tools that can write. Written here rather than glossed: this is
//! the weaker of the two paths.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, oneshot, Mutex};

/// Tools that can change something. If the agent has not disabled these, the
/// reply is refused — see [`guards_hold`].
///
/// Not the whole list on purpose. A complete list would need updating every
/// time the vendor adds a tool, and would pass by accident the day they rename
/// one; these four are the ones whose presence means the model can write to the
/// disk, run a command, or reach a database on its own.
const MUST_BE_DISABLED: [&str; 4] = ["bash", "edit", "create", "sql"];

/// What a `session/update` notification meant to us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// Text for the person.
    Chunk(String),
    /// The agent's own commentary, which must not reach the chat.
    Plumbing(String),
    /// The tools it says it disabled, from the `Info:` line.
    Disabled(Vec<String>),
    /// The model called a tool. Must never happen.
    ToolCall(String),
    /// Anything else: commands, config, titles, token counts.
    Ignored,
}

/// Is this chunk the agent talking about itself rather than answering?
///
/// Narrow by design — the two forms actually observed, matched at the start of
/// the chunk. A reply that happens to *discuss* disabled tools is an answer and
/// stays one.
pub fn is_plumbing(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("Info: Disabled tools:") || t.starts_with("Info: Unknown tool name")
}

/// The tool names out of an `Info: Disabled tools: a, b, c` line.
pub fn disabled_tools(text: &str) -> Option<Vec<String>> {
    let rest = text.trim_start().strip_prefix("Info: Disabled tools:")?;
    Some(
        rest.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// Does what the agent disabled cover everything that could change something?
///
/// `None` for "it never said", which is itself a failure: the line is how this
/// path knows the flags worked at all, so its absence is not reassurance.
pub fn guards_hold(disabled: Option<&Vec<String>>) -> Result<(), String> {
    let Some(disabled) = disabled else {
        return Err(
            "Copilot CLI did not report disabling its tools, so this answer was stopped. \
             The assistant is only allowed to run with none."
                .to_string(),
        );
    };
    let missing: Vec<&str> = MUST_BE_DISABLED
        .iter()
        .copied()
        .filter(|t| !disabled.iter().any(|d| d == t))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Copilot CLI left {} available to the model ({}). The assistant is not allowed any \
         tool that can change something, so this answer was stopped. Please report this — \
         the flags that remove them have changed.",
        if missing.len() == 1 { "a tool" } else { "tools" },
        missing.join(", ")
    ))
}

/// Read one `session/update`'s `update` object.
pub fn classify(update: &Value) -> Update {
    match update["sessionUpdate"].as_str() {
        Some("agent_message_chunk") => {
            let text = update["content"]["text"].as_str().unwrap_or("");
            if is_plumbing(text) {
                match disabled_tools(text) {
                    Some(names) => Update::Disabled(names),
                    None => Update::Plumbing(text.to_string()),
                }
            } else {
                Update::Chunk(text.to_string())
            }
        }
        Some("tool_call") => Update::ToolCall(
            update["title"]
                .as_str()
                .or_else(|| update["toolCallId"].as_str())
                .unwrap_or("?")
                .to_string(),
        ),
        _ => Update::Ignored,
    }
}

// ------------------------------------------------------------------- the client

/// How long a turn may produce nothing at all before we give up on it.
///
/// Generous: a model that is thinking is not a model that has died, and the
/// person can always press Stop. This is the backstop for an agent that has
/// stopped speaking without ending the turn.
const TURN_SILENCE: std::time::Duration = std::time::Duration::from_secs(300);

/// A CLI held open, with one ACP session on it.
pub struct Agent {
    stdin: Mutex<tokio::process::ChildStdin>,
    child: Mutex<tokio::process::Child>,
    next_id: AtomicI64,
    /// Requests we are waiting on, by id.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    /// Every `session/update`, for whichever turn is in flight.
    updates: broadcast::Sender<Value>,
    session: String,
    /// What the agent said it disabled, once, for this whole session.
    ///
    /// Announced on the **first turn only** — found by a live test whose second
    /// turn was refused for not repeating it. So it is remembered here: the
    /// session establishes the fact, and every later turn still refuses on a
    /// `tool_call`, which is the part that could change mid-session.
    disabled: Mutex<Option<Vec<String>>>,
    /// What system prompt this session was primed with. When the schema
    /// changes, the session is stale — see `Agent::primed_with`.
    system: String,
}

impl Agent {
    /// Spawn a CLI, shake hands, and open a session.
    ///
    /// `system` is sent as the first turn's preamble by the caller, not here:
    /// Copilot has no system-prompt flag, so the rules travel in the first
    /// question and the session remembers them.
    pub async fn start(
        program: &str,
        args: &[String],
        cwd: &std::path::Path,
        system: String,
    ) -> Result<Self, String> {
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    format!("`{program}` was not found on your PATH.")
                }
                _ => format!("Could not start `{program}`: {e}"),
            })?;

        let stdin = child.stdin.take().ok_or("The CLI has no stdin.")?;
        let stdout = child.stdout.take().ok_or("The CLI has no stdout.")?;

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (updates, _) = broadcast::channel(512);

        // One reader for the process's whole life. It owns the dispatch: a
        // response goes to whoever is waiting on that id, a notification goes
        // to the broadcast, and **a request from the agent is answered with an
        // error** — we advertise no capabilities, so anything it asks for is
        // something we cannot do, and leaving it unanswered would hang it.
        {
            let pending = pending.clone();
            let updates = updates.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if let Some(id) = v["id"].as_i64() {
                        if v.get("result").is_some() || v.get("error").is_some() {
                            if let Some(tx) = pending.lock().await.remove(&id) {
                                let _ = tx.send(v);
                            }
                            continue;
                        }
                        // A request from the agent. Refused, not ignored.
                        let _ = updates.send(json!({ "__refuse": id }));
                        continue;
                    }
                    if v["method"] == "session/update" {
                        let _ = updates.send(v["params"]["update"].clone());
                    }
                }
            });
        }

        let agent = Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
            pending,
            updates,
            session: String::new(),
            disabled: Mutex::new(None),
            system,
        };

        agent
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } }
                }),
            )
            .await?;

        let new = agent
            .request(
                "session/new",
                json!({ "cwd": cwd.display().to_string(), "mcpServers": [] }),
            )
            .await?;
        let session = new["sessionId"]
            .as_str()
            .ok_or("The CLI opened no session.")?
            .to_string();

        Ok(Self { session, ..agent })
    }

    /// Was this session primed with the same context? If not it is stale.
    ///
    /// The schema rides in the first turn, so a session that was opened while
    /// you were in one database is answering about that one. Comparing the
    /// whole prompt catches every reason it could have changed — a different
    /// connection, a different schema, a table newly expanded — without
    /// enumerating them.
    pub fn primed_with(&self, system: &str) -> bool {
        self.system == system
    }

    /// Send a request and wait for its response.
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.write(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        let answer = tokio::time::timeout(std::time::Duration::from_secs(60), rx)
            .await
            .map_err(|_| format!("The CLI did not answer `{method}`."))?
            .map_err(|_| "The CLI stopped before answering.".to_string())?;
        if let Some(message) = answer["error"]["message"].as_str() {
            return Err(format!("{message}."));
        }
        Ok(answer["result"].clone())
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn write(&self, v: Value) -> Result<(), String> {
        let mut line = serde_json::to_string(&v).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Could not write to the CLI: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("Could not write to the CLI: {e}"))
    }

    /// Ask one question, streaming the answer through `emit`.
    ///
    /// `Ok(None)` means the turn was cancelled. The session stays usable either
    /// way, which is the point of cancelling politely rather than killing.
    pub async fn prompt(
        &self,
        text: &str,
        cancelled: &crate::session::CliCancel,
        mut emit: impl FnMut(Outcome),
    ) -> Result<Option<String>, String> {
        // Asked to stop before we even sent the question. Nothing is spent.
        if cancelled.was_asked() {
            return Ok(None);
        }
        // Subscribed **before** the prompt is sent, or the first chunks of a
        // fast answer are gone before anyone is listening.
        let mut updates = self.updates.subscribe();

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.write(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": self.session,
                "prompt": [{ "type": "text", "text": text }]
            }
        }))
        .await?;

        let notified = cancelled.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        tokio::pin!(rx);

        let mut cancelling = false;

        loop {
            tokio::select! {
                answer = &mut rx => {
                    let answer = answer.map_err(|_| "The CLI stopped mid-answer.".to_string())?;
                    if let Some(message) = answer["error"]["message"].as_str() {
                        return Err(format!("{message}."));
                    }
                    let stop = answer["result"]["stopReason"].as_str().unwrap_or("end_turn");
                    if cancelling || stop == "cancelled" {
                        return Ok(None);
                    }
                    // Checked when the turn ends as well as when the line
                    // arrives, so a turn that never mentioned its tools is
                    // refused rather than quietly trusted.
                    guards_hold(self.disabled.lock().await.as_ref())?;
                    return Ok(Some(stop.to_string()));
                }
                update = updates.recv() => {
                    let update = match update {
                        Ok(u) => u,
                        // The reader outran us. Dropping text would silently
                        // truncate an answer, so it is reported.
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            emit(Outcome::Failed(format!(
                                "{n} pieces of the answer were dropped before they could be shown."
                            )));
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err("The CLI stopped talking.".into());
                        }
                    };
                    if let Some(refuse) = update["__refuse"].as_i64() {
                        // Something we advertised no capability for.
                        let _ = self.write(json!({
                            "jsonrpc": "2.0", "id": refuse,
                            "error": { "code": -32601, "message": "This client offers no such capability." }
                        })).await;
                        continue;
                    }
                    match classify(&update) {
                        Update::Chunk(text) => emit(Outcome::Text(text)),
                        Update::Disabled(names) => {
                            if let Err(message) = guards_hold(Some(&names)) {
                                let _ = self.notify("session/cancel", json!({ "sessionId": self.session })).await;
                                return Err(message);
                            }
                            *self.disabled.lock().await = Some(names);
                        }
                        Update::ToolCall(name) => {
                            let _ = self.notify("session/cancel", json!({ "sessionId": self.session })).await;
                            return Err(format!(
                                "Copilot CLI tried to use a tool ({name}). The assistant has no \
                                 tools and runs nothing, so this answer was stopped."
                            ));
                        }
                        Update::Plumbing(_) | Update::Ignored => {}
                    }
                }
                _ = &mut notified, if !cancelling => {
                    // Polite: the turn ends, the session lives, and the next
                    // question does not pay to start a process again.
                    cancelling = true;
                    let _ = self.notify("session/cancel", json!({ "sessionId": self.session })).await;
                }
                _ = tokio::time::sleep(TURN_SILENCE) => {
                    return Err("The CLI stopped answering.".into());
                }
            }
        }
    }

    /// Stop the process. Called when the session is replaced or the app is done.
    pub async fn close(&self) {
        let _ = self
            .notify("session/cancel", json!({ "sessionId": self.session }))
            .await;
        let _ = self.child.lock().await.kill().await;
    }
}

/// What a turn produces as it runs.
pub enum Outcome {
    Text(String),
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // The two Info lines are quoted exactly as the real CLI produced them on
    // 2026-10-01. If a vendor rewording breaks these, it should break a test
    // rather than start leaking plumbing into someone's chat.
    const DISABLED_LINE: &str = "Info: Disabled tools: bash, create, dynamic_workflows_manage, \
         edit, glob, grep, list_agents, list_bash, read_agent, read_bash, run_dynamic_workflow, \
         sql, stop_bash, task, view, web_fetch, write_agent";
    const UNKNOWN_LINE: &str =
        "Info: Unknown tool name in the tool allowlist: \"__db_query_no_tools__\"";

    fn chunk(text: &str) -> Value {
        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } })
    }

    #[test]
    fn the_agents_own_commentary_never_reaches_the_chat() {
        assert!(is_plumbing(DISABLED_LINE));
        assert!(is_plumbing(UNKNOWN_LINE));
        assert!(matches!(classify(&chunk(UNKNOWN_LINE)), Update::Plumbing(_)));
        assert!(matches!(
            classify(&chunk(DISABLED_LINE)),
            Update::Disabled(_)
        ));
    }

    /// An answer that *talks about* disabled tools is still an answer.
    #[test]
    fn an_answer_mentioning_tools_is_not_plumbing() {
        for text in [
            "Info about your schema: the orders table has 4 columns.",
            "You could disable tools like bash, but that is unrelated.",
            "The Disabled tools: label appears in Copilot's own output.",
        ] {
            assert!(!is_plumbing(text), "{text}");
            assert!(matches!(classify(&chunk(text)), Update::Chunk(_)), "{text}");
        }
    }

    #[test]
    fn the_disabled_line_yields_its_tool_names() {
        let names = disabled_tools(DISABLED_LINE).expect("a list");
        for expected in ["bash", "create", "edit", "sql", "web_fetch"] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        assert_eq!(names.len(), 17);
    }

    #[test]
    fn a_full_disabled_list_passes_the_guard() {
        let names = disabled_tools(DISABLED_LINE).unwrap();
        assert!(guards_hold(Some(&names)).is_ok());
    }

    /// The case the guard exists for: the flags stopped working and `bash` is
    /// back. The message has to name it.
    #[test]
    fn a_tool_that_can_write_fails_the_guard_by_name() {
        let names: Vec<String> = ["create", "edit", "sql"].iter().map(|s| s.to_string()).collect();
        let err = guards_hold(Some(&names)).expect_err("bash is missing from the disabled list");
        assert!(err.contains("bash"), "{err}");
        assert!(err.contains("stopped"), "{err}");
    }

    /// Saying nothing is not the same as saying nothing is wrong.
    #[test]
    fn a_turn_that_never_mentioned_its_tools_is_refused() {
        let err = guards_hold(None).expect_err("silence is not reassurance");
        assert!(err.contains("did not report"), "{err}");
    }

    #[test]
    fn a_tool_call_is_recognised_however_it_is_labelled() {
        assert_eq!(
            classify(&json!({ "sessionUpdate": "tool_call", "title": "Run bash" })),
            Update::ToolCall("Run bash".into())
        );
        assert_eq!(
            classify(&json!({ "sessionUpdate": "tool_call", "toolCallId": "tc_1" })),
            Update::ToolCall("tc_1".into())
        );
    }

    /// Everything else the real session sent, which must not be mistaken for
    /// text: these arrived in the measured run.
    #[test]
    fn session_housekeeping_is_ignored() {
        for kind in [
            "available_commands_update",
            "session_info_update",
            "config_option_update",
            "usage_update",
        ] {
            assert_eq!(
                classify(&json!({ "sessionUpdate": kind, "used": 1553 })),
                Update::Ignored,
                "{kind}"
            );
        }
    }
}
