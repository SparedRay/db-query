//! What a schema actually costs in a prompt.
//!
//! Stage 30 §3. The assistant's column limit was 60 tables, a number borrowed
//! from [`schema::TABLE_DETAIL_BUDGET`] — which was chosen to bound *round
//! trips* on an engine that describes one table at a time, not to bound a
//! prompt. MySQL now describes a whole database in one query, so the round-trip
//! reason is gone and the prompt limit was left standing on it.
//!
//! These are measurements kept as tests, in the manner of `measure_sqlfiles`:
//! the numbers the design is built on should fail here if rendering ever
//! changes its mind about them. Run with
//! `cargo test --test measure_prompt -- --ignored --nocapture`.

use std::collections::HashMap;

use db_query_lib::assistant::{ranked_note, reckoning, render_schema};
use db_query_lib::relevance;
use db_query_lib::schema::{ColumnInfo, DbSchema, TableRef};

/// A database shaped like the one in the field report: ~400 tables and views,
/// a dozen columns each, names and types of ordinary length.
fn wide_schema(tables: usize, cols: usize) -> DbSchema {
    let types = [
        ("id", "int", false, Some("PRI")),
        ("party_id", "int", false, Some("MUL")),
        ("status_id", "int", true, Some("MUL")),
        ("balance", "decimal(12,2)", true, None),
        ("original_balance", "decimal(12,2)", true, None),
        ("reserve_balance", "decimal(12,2)", true, None),
        ("account_number", "varchar(64)", true, None),
        ("description", "varchar(255)", true, None),
        ("is_active", "tinyint(1)", false, None),
        ("created_at", "datetime", false, None),
        ("updated_at", "datetime", true, None),
        ("deleted_at", "datetime", true, None),
    ];
    let mut columns = HashMap::new();
    let mut refs = Vec::new();
    for i in 0..tables {
        // Names of a realistic length, a tenth of them views.
        let name = format!("enrolled_item_detail_{i:04}");
        refs.push(TableRef {
            name: name.clone(),
            kind: if i % 10 == 9 { "VIEW" } else { "BASE TABLE" }.into(),
        });
        columns.insert(
            name,
            (0..cols)
                .map(|c| {
                    let (n, t, nullable, key) = types[c % types.len()];
                    ColumnInfo {
                        name: n.to_string(),
                        data_type: t.to_string(),
                        nullable,
                        key: key.map(str::to_string),
                    }
                })
                .collect(),
        );
    }
    DbSchema {
        tables: Some(refs),
        columns,
        routines: None,
    }
}

#[test]
#[ignore = "a measurement, not an assertion"]
fn what_a_schema_costs_in_a_prompt() {
    let schema = wide_schema(400, 12);
    println!("400 tables x 12 columns, one prompt");
    // Spanning the old table limit (60 tables of this width is ~22,700
    // characters of columns) up to the whole schema.
    for budget in [0usize, 23_000, 45_000, 60_000, 115_000, 200_000] {
        let started = std::time::Instant::now();
        let out = render_schema("appdata", &schema, &[], budget, None);
        let took = started.elapsed();
        let chars = out.text.chars().count();
        println!(
            "  budget {budget:>7}: {chars:>7} chars  ~{:>6} tokens  \
             {:>3} described, {:>3} by name  in {:>6.2}ms",
            chars / 4,
            400 - out.elided.len(),
            out.elided.len(),
            took.as_secs_f64() * 1000.0
        );
    }

    // What one table costs, so a budget in characters can be read back as a
    // number of tables.
    let one = render_schema("appdata", &wide_schema(1, 12), &[], 1 << 20, None);
    let bare = render_schema("appdata", &wide_schema(1, 12), &[], 0, None);
    let head = render_schema("appdata", &wide_schema(0, 12), &[], 0, None);
    let base = head.text.chars().count();
    println!(
        "  one table with columns: {} chars; named alone: {} chars",
        one.text.chars().count() - base,
        // The bare form carries the elision sentence, charged once.
        bare.text.chars().count() - base,
    );
}

/// What ranking one question costs, and what the log line it produces looks
/// like — the two things that run on **every** question rather than once.
#[test]
#[ignore = "a measurement, not an assertion"]
fn what_ranking_a_question_costs() {
    let schema = wide_schema(400, 12);
    let question = "for one member, list every enrolled item with the party name, \
                    servicing status, supplier status, current and original supplier, \
                    balances and reserve balance";

    let started = std::time::Instant::now();
    let asked = relevance::words(question);
    let words_took = started.elapsed();

    let started = std::time::Instant::now();
    let order = relevance::order(
        "appdata",
        &schema,
        &relevance::Signals {
            question: Some(question),
            ..Default::default()
        },
    );
    let order_took = started.elapsed();

    // The extra pass the log line costs. It is per question, so it has to be
    // cheap enough not to be noticed beside a model's first token.
    let started = std::time::Instant::now();
    let (landed, missed) = relevance::recognised(&schema, &asked);
    let recognised_took = started.elapsed();

    let started = std::time::Instant::now();
    let line = reckoning("appdata", &schema);
    let reckoning_took = started.elapsed();

    println!("400 tables x 12 columns, one question");
    println!("  words       {:>7.2}ms", words_took.as_secs_f64() * 1000.0);
    println!("  order       {:>7.2}ms", order_took.as_secs_f64() * 1000.0);
    println!(
        "  recognised  {:>7.2}ms  ({} landed, {missed} did not)",
        recognised_took.as_secs_f64() * 1000.0,
        landed.len()
    );
    println!(
        "  reckoning   {:>7.2}ms  (diagnostics only, three renders)",
        reckoning_took.as_secs_f64() * 1000.0
    );

    let out = render_schema("appdata", &schema, &order, 60_000, None);
    let note = ranked_note(
        "appdata",
        &landed,
        missed,
        &order,
        &out,
        order_took + recognised_took,
    );
    println!("\n  reckoning line: {}", line.trim_end());
    println!("  log line ({} chars):\n    {note}", note.len());
}
