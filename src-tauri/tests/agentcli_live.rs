//! The agent-CLI provider, against a real CLI.
//!
//! `#[ignore]` throughout, like every other live test here: these need a CLI
//! installed *and* signed in, and one of them spends a request on the person's
//! own subscription. `mise run test-cli` runs them.
//!
//! What makes them worth their cost is that the whole design rests on
//! behaviour nobody documents — see `agentcli.rs`'s module docs. The unit tests
//! pin the shapes we captured; these check the shapes are still what the
//! vendor produces.

use db_query_lib::agentcli::{self, Recipe};
use db_query_lib::assistant::{ChatMessage, StreamEvent};

const SYSTEM: &str = "You are a test fixture for a database client. \
                      Answer in as few words as possible. You have no tools.";

fn ask(text: &str) -> Vec<ChatMessage> {
    vec![ChatMessage {
        role: "user".into(),
        content: text.into(),
    }]
}

/// Which recipe to exercise. Copilot by default because it is the one the
/// person said would be the main driver; `CLI_RECIPE=claude` switches.
fn recipe() -> Recipe {
    match std::env::var("CLI_RECIPE").unwrap_or_default().as_str() {
        "claude" => Recipe::Claude,
        _ => Recipe::Copilot,
    }
}

/// **Readiness costs nothing.** No model call, no billable request — which is
/// the whole reason the integration can be warmed at startup.
///
/// Run it twice and watch the clock: if this ever starts taking seconds, or
/// showing up on a usage page, the probe has stopped being free and the
/// startup warming has to go.
#[tokio::test]
#[ignore = "needs a CLI installed"]
async fn a_probe_answers_without_asking_a_model_anything() {
    let started = std::time::Instant::now();
    let status = agentcli::probe(recipe()).await;
    let took = started.elapsed();

    println!("{:?} -> {status:?} in {took:?}", recipe());
    assert!(
        status.installed,
        "{} is not installed: {}",
        recipe().label(),
        status.detail
    );
    assert!(
        status.signed_in,
        "not signed in, so the rest of this file will not run: {}",
        status.detail
    );
    assert!(status.detail.is_empty(), "a ready CLI needs no explanation");
}

/// One real question, and **the invariant holds against the real CLI**.
///
/// `run` fails the reply when the CLI reports a non-empty tool set, so a plain
/// answer with no `Failed` event *is* the assertion that no tools were
/// offered — the 21 of them that `--available-tools=` leaves in place would
/// stop this test.
///
/// Spends one request.
#[tokio::test]
#[ignore = "spends a request on the person's subscription"]
async fn a_real_cli_answers_with_no_tools_in_reach() {
    let cancel = tokio::sync::Notify::new();
    let events = std::sync::Mutex::new(Vec::new());

    agentcli::run(
        recipe(),
        SYSTEM,
        &ask("Reply with exactly: OK"),
        &cancel,
        |e| events.lock().unwrap().push(e),
    )
    .await
    .expect("the CLI should have started");

    let events = events.into_inner().unwrap();
    println!("{events:#?}");

    let failed: Vec<&StreamEvent> = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Failed { .. }))
        .collect();
    assert!(
        failed.is_empty(),
        "the reply failed, which is also how a leaked tool set reports itself: {failed:?}"
    );

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Text { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.to_uppercase().contains("OK"), "got {text:?}");
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Done { .. })),
        "a finished reply says so"
    );
}

/// **The starting state is real.** Copilot says nothing for its first couple of
/// seconds, so the UI needs an event before the first token or it shows an
/// empty bubble and looks broken.
///
/// Spends one request.
#[tokio::test]
#[ignore = "spends a request on the person's subscription"]
async fn something_arrives_before_the_first_word() {
    let cancel = tokio::sync::Notify::new();
    let marks = std::sync::Mutex::new(Vec::new());
    let started = std::time::Instant::now();

    agentcli::run(
        recipe(),
        SYSTEM,
        &ask("Reply with exactly: OK"),
        &cancel,
        |e| marks.lock().unwrap().push((started.elapsed(), e)),
    )
    .await
    .unwrap();

    let marks = marks.into_inner().unwrap();
    let first_start = marks
        .iter()
        .find(|(_, e)| matches!(e, StreamEvent::Started))
        .map(|(at, _)| *at)
        .expect("a Started event");
    let first_text = marks
        .iter()
        .find(|(_, e)| matches!(e, StreamEvent::Text { .. }))
        .map(|(at, _)| *at)
        .expect("some text");

    println!("started at {first_start:?}, first text at {first_text:?}");
    assert!(
        first_start <= first_text,
        "the starting event has to come first, or it is not worth having"
    );
}

/// **Cancel kills the process.** Milestone C8.
///
/// A CLI reply that is merely abandoned keeps talking to a server and spending
/// the person's quota, which is why this needs a kill rather than a dropped
/// future. The reply is cancelled as soon as any event arrives, and what proves
/// the point is the absence of `Done`: the run returned because it was stopped,
/// not because the CLI finished.
///
/// Spends one request — a cancelled one still counts.
#[tokio::test]
#[ignore = "spends a request on the person's subscription"]
async fn cancelling_stops_the_reply_rather_than_waiting_it_out() {
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let events = std::sync::Mutex::new(Vec::new());
    let fired = std::sync::atomic::AtomicBool::new(false);

    let started = std::time::Instant::now();
    agentcli::run(
        recipe(),
        SYSTEM,
        // Long enough that it cannot have finished before the cancel lands.
        &ask("Write a 400 word essay about database indexes."),
        &cancel,
        |e| {
            events.lock().unwrap().push(e);
            if !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                cancel.notify_waiters();
            }
        },
    )
    .await
    .unwrap();
    let took = started.elapsed();

    let events = events.into_inner().unwrap();
    println!("{took:?}, {} events", events.len());
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Done { .. })),
        "a cancelled reply never reports itself finished: {events:#?}"
    );
}
