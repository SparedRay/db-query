//! The one question the unit tests cannot answer: **is the request shape right?**
//!
//! Everything else about the assistant is pure and tested offline — the SSE
//! decoder, the prompt, the error messages. What none of that proves is that
//! the API accepts our body, because a wrong field name comes back as a 400
//! rather than as a compile error.
//!
//! Ignored by default and needs a key:
//!
//!     ANTHROPIC_API_KEY=sk-ant-... cargo test --manifest-path src-tauri/Cargo.toml \
//!         --test assistant_live -- --ignored --nocapture
//!
//! It costs a few cents to run.

use db_query_lib::assistant::{self, ChatMessage, Provider, StreamEvent};

fn key() -> String {
    std::env::var("ANTHROPIC_API_KEY")
        .expect("set ANTHROPIC_API_KEY to run this; it is ignored by default")
}

#[tokio::test]
#[ignore]
async fn the_api_accepts_our_request_and_streams_sql_back() {
    let system = assistant::system_prompt(
        "8.4.0",
        Some("TABLE `users` (id int NOT NULL PK, email varchar(190) NOT NULL UNIQUE)"),
    );
    let messages = vec![ChatMessage {
        role: "user".into(),
        content: "Write a query counting the users. One sql block, no commentary.".into(),
    }];

    let response = reqwest::Client::new()
        .post(Provider::Anthropic.endpoint(assistant::ANTHROPIC_BASE))
        .header("x-api-key", key())
        .header("anthropic-version", assistant::API_VERSION)
        .header("content-type", "application/json")
        .json(&assistant::request(
            Provider::Anthropic,
            assistant::ANTHROPIC_MODEL,
            &system,
            &messages,
        ))
        .send()
        .await
        .expect("request failed");

    let status = response.status();
    assert!(
        status.is_success(),
        "{}",
        assistant::http_error(status.as_u16(), &response.text().await.unwrap_or_default())
    );

    let mut response = response;
    let mut decoder = assistant::SseDecoder::new(Provider::Anthropic);
    let mut answer = String::new();
    let mut done = false;

    while let Ok(Some(bytes)) = response.chunk().await {
        for event in decoder.push(&String::from_utf8_lossy(&bytes)) {
            match event {
                StreamEvent::Text { delta } => answer.push_str(&delta),
                StreamEvent::Done { .. } => done = true,
                StreamEvent::Failed { message } => panic!("stream failed: {message}"),
                StreamEvent::Thinking { .. } => {}
            }
        }
    }

    assert!(done, "the stream never reported a stop reason");
    println!("--- answer ---\n{answer}\n--------------");
    assert!(
        answer.to_ascii_lowercase().contains("select"),
        "expected SQL, got: {answer}"
    );
    // The prompt asks for fenced blocks, because the UI turns them into buttons.
    assert!(
        answer.contains("```"),
        "expected a fenced block, got: {answer}"
    );
}

/// A bad key must produce the message the settings dialog promises, not a raw
/// JSON body.
#[tokio::test]
#[ignore]
async fn a_bad_key_is_reported_readably() {
    let messages = vec![ChatMessage {
        role: "user".into(),
        content: "hi".into(),
    }];
    let response = reqwest::Client::new()
        .post(Provider::Anthropic.endpoint(assistant::ANTHROPIC_BASE))
        .header("x-api-key", "sk-ant-obviously-not-valid")
        .header("anthropic-version", assistant::API_VERSION)
        .header("content-type", "application/json")
        .json(&assistant::request(
            Provider::Anthropic,
            assistant::ANTHROPIC_MODEL,
            "sys",
            &messages,
        ))
        .send()
        .await
        .expect("request failed");

    let status = response.status().as_u16();
    assert_eq!(status, 401, "expected the key to be rejected");
    let message = assistant::http_error(status, &response.text().await.unwrap_or_default());
    assert!(message.contains("API key"), "{message}");
    assert!(message.contains("Settings"), "{message}");
}

/// The same flow against a **local** OpenAI-compatible server.
///
/// Ignored and opt-in, because it needs something listening. It is the cheapest
/// possible proof that the second adapter is real rather than plausible:
///
///     ollama serve &
///     ollama pull llama3.1
///     OLLAMA_MODEL=llama3.1 cargo test --manifest-path src-tauri/Cargo.toml \
///         --test assistant_live -- --ignored ollama --nocapture
#[tokio::test]
#[ignore]
async fn a_local_openai_compatible_server_streams_back() {
    let model = std::env::var("OLLAMA_MODEL")
        .expect("set OLLAMA_MODEL to a model you have pulled, e.g. llama3.1");
    let base =
        std::env::var("OLLAMA_BASE").unwrap_or_else(|_| "http://localhost:11434/v1".to_string());

    assert!(
        assistant::is_local(&base),
        "this test is for a local server"
    );
    assert!(
        !Provider::OpenAiCompatible.requires_key(&base),
        "a local server should not be asked for a key"
    );

    let system = assistant::system_prompt("8.4.0", Some("TABLE `users` (id int NOT NULL PK)"));
    let messages = vec![ChatMessage {
        role: "user".into(),
        content: "Write a query counting the users. One sql block, no commentary.".into(),
    }];

    let response = reqwest::Client::new()
        .post(Provider::OpenAiCompatible.endpoint(&base))
        .header("content-type", "application/json")
        .json(&assistant::request(
            Provider::OpenAiCompatible,
            &model,
            &system,
            &messages,
        ))
        .send()
        .await
        .expect("is the server running?");

    let status = response.status();
    assert!(
        status.is_success(),
        "{}",
        assistant::http_error(status.as_u16(), &response.text().await.unwrap_or_default())
    );

    let mut response = response;
    let mut decoder = assistant::SseDecoder::new(Provider::OpenAiCompatible);
    let mut answer = String::new();
    while let Ok(Some(bytes)) = response.chunk().await {
        for event in decoder.push(&String::from_utf8_lossy(&bytes)) {
            match event {
                StreamEvent::Text { delta } => answer.push_str(&delta),
                StreamEvent::Failed { message } => panic!("stream failed: {message}"),
                _ => {}
            }
        }
    }

    println!("--- answer ---\n{answer}\n--------------");
    assert!(
        answer.to_ascii_lowercase().contains("select"),
        "expected SQL, got: {answer}"
    );
}
