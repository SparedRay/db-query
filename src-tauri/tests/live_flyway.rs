//! The Flyway command line, actually run.
//!
//! `#[ignore]`d like the MySQL and Elasticsearch suites: it needs
//! `mise run flyway-up`, which fetches the pinned CLI and prepares the two
//! fixture databases on the MySQL container.
//!
//! What the unit tests in `flywacli.rs` cannot answer: that the arguments we
//! build are arguments *Flyway accepts*, that the working directory makes a
//! relative `locations` resolve, and that `-environment=` really does pick the
//! database. Every one of those is a thing the JSON parser would happily agree
//! about while the app pointed at the wrong server.

use std::path::PathBuf;

use db_query_lib::flywaycli;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .to_path_buf()
}

/// The pinned CLI, or whatever `FLYWAY_BIN` names — so a developer with their
/// own install does not have to download a second copy.
fn program() -> String {
    if let Ok(p) = std::env::var("FLYWAY_BIN") {
        return p;
    }
    repo()
        .join("dev/.flyway/flyway-13.5.0/flyway")
        .to_string_lossy()
        .into_owned()
}

fn project() -> PathBuf {
    repo().join("dev/flyway/flyway.toml")
}

async fn info(environment: &str) -> flywaycli::Finished {
    flywaycli::run(&program(), &project(), environment, "info", vec![])
        .await
        .expect("flyway did not run — is `mise run flyway-up` done?")
}

/// The whole point of §3.4: `locations = ["filesystem:migrations"]` is
/// relative, and it resolves because the working directory is the project's
/// folder. Get that wrong and Flyway finds no migrations at all — which it
/// reports as a *successful* run of an empty project, not as an error.
#[tokio::test]
#[ignore]
async fn the_relative_locations_resolve_and_every_migration_is_found() {
    let out = info("development").await;
    assert!(out.ok(), "exit {:?}: {}", out.code, out.stderr);

    let m = flywaycli::migrations(&out.stdout).expect("an info list");
    let versions: Vec<&str> = m.iter().filter_map(|x| x.version.as_deref()).collect();
    assert_eq!(
        versions,
        ["1", "2", "3", "4"],
        "an empty list here means the working directory was wrong, not that \
         the project is empty"
    );
    assert_eq!(m[0].description, "create widgets");
}

/// `filepath` is absolute and real, which is what makes "click a migration to
/// read it" a file read rather than a folder search.
#[tokio::test]
#[ignore]
async fn a_migration_can_be_read_from_the_path_flyway_gives() {
    let out = info("development").await;
    let m = flywaycli::migrations(&out.stdout).unwrap();
    let path = m[0].filepath.as_deref().expect("a filepath");

    let sql = std::fs::read_to_string(path).expect("the path Flyway gave is a file we can read");
    assert!(sql.contains("CREATE TABLE widgets"), "{sql}");
}

/// **The environment is the database.** If `-environment=` did not select it,
/// every guard in this stage would be guarding nothing.
#[tokio::test]
#[ignore]
async fn the_environment_chooses_which_database_is_reported() {
    for (env, db) in [("development", "flyway_dev"), ("qa", "flyway_qa")] {
        let out = info(env).await;
        let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(
            v.get("database").and_then(|d| d.as_str()),
            Some(db),
            "-environment={env} reported the wrong database"
        );
    }
}

/// An environment the project does not define is Flyway's complaint, in
/// Flyway's words — not a panic and not our paraphrase.
#[tokio::test]
#[ignore]
async fn an_unknown_environment_comes_back_as_flyways_own_refusal() {
    let out = flywaycli::run(&program(), &project(), "nope", "info", vec![])
        .await
        .expect("flyway ran");
    assert!(
        !out.ok(),
        "an unknown environment must not look like success"
    );

    let c = flywaycli::complaint(&out.stdout).expect("a complaint in the JSON");
    assert!(
        c.message.to_lowercase().contains("nope"),
        "the message should name what was wrong: {}",
        c.message
    );
}

/// The diagnostic report, against a Flyway that is really there.
///
/// It exists to be pasted into a bug report, so the thing worth proving is
/// that it contains the three answers somebody needs: which command, where it
/// was found, and what Flyway itself said. The unit tests cover the shape when
/// it is *not* there, which is the commoner case and the easier one to fake.
#[tokio::test]
#[ignore]
async fn the_report_carries_flyways_own_version() {
    let text = flywaycli::probe(&program()).await;
    println!("{text}");

    assert!(text.contains("exit       0"), "{text}");
    assert!(text.contains("--- stdout ---"), "{text}");
    assert!(
        text.contains("Flyway") && text.contains("13.5.0"),
        "the version is the point of it: {text}"
    );
}

/// The whole report, end to end, against a Flyway that is really there.
#[tokio::test]
#[ignore]
async fn the_whole_report_reads_as_one_document() {
    use db_query_lib::logbook;
    logbook::info("app", "db-query 0.4.2 starting on linux x86_64");
    logbook::warn(
        "mysql",
        "Access denied for user 'root'@'localhost' (using password: YES)",
    );
    logbook::error("mysql", "connecting to mysql://root:hunter2@db:3306/app");

    let text = logbook::report(&flywaycli::probe(&program()).await);
    println!("{text}");

    assert!(text.contains("=== Flyway ==="), "{text}");
    assert!(text.contains("13.5.0"), "{text}");
    assert!(
        text.contains("(using password: YES)"),
        "the useful line survives"
    );
    assert!(!text.contains("hunter2"), "and the secret does not");
}

// -------------------------------------------------------------- applying

/// Reset one fixture environment to empty, so an apply test starts from
/// nothing rather than from whatever the last run left.
fn reset(db: &str) {
    let out = std::process::Command::new("podman")
        .args([
            "exec",
            "-i",
            "db-query-mysql",
            "mysql",
            "-uroot",
            "-pdevpassword",
            "-e",
        ])
        .arg(format!(
            "DROP DATABASE IF EXISTS {db}; CREATE DATABASE {db};"
        ))
        .output()
        .expect("podman exec");
    assert!(out.status.success(), "could not reset {db}");
}

/// **Phase 2's whole claim, against a real database.**
///
/// The fixture's fourth migration is broken on purpose, so one run covers both
/// halves: three apply, the fourth fails, and Flyway's own words say which one
/// and why.
#[tokio::test]
#[ignore]
async fn applying_runs_the_pending_migrations_and_stops_at_the_broken_one() {
    reset("flyway_qa");

    let out = flywaycli::run(&program(), &project(), "qa", "migrate", vec![])
        .await
        .expect("flyway ran");

    // It failed, and it failed on V4 rather than somewhere vague.
    assert!(!out.ok(), "the fixture's V4 is broken on purpose");
    let c = flywaycli::complaint(&out.stdout).expect("Flyway complained");
    assert_eq!(c.code.as_deref(), Some("FAILED_VERSIONED_MIGRATION"));
    assert!(
        c.message.contains("V4__deliberately_broken.sql"),
        "{}",
        c.message
    );
    // Flyway's own words, kept: the line a user acts on is the SQL error.
    assert!(c.message.contains("Can't DROP 'weight'"), "{}", c.message);

    // Three ran before it did, and Flyway says so in the field the command reads.
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["migrationsExecuted"], 3);

    // And the state afterwards is what the pane will show: three done, one
    // failed and blocking.
    let after = flywaycli::migrations(&info("qa").await.stdout).unwrap();
    let group = |v: &str| {
        after
            .iter()
            .find(|m| m.version.as_deref() == Some(v))
            .unwrap()
            .group
    };
    assert_eq!(group("1"), flywaycli::Group::Done);
    assert_eq!(group("3"), flywaycli::Group::Done);
    assert_eq!(group("4"), flywaycli::Group::Failed);
}

/// A second apply is refused outright while a failure sits in the history —
/// which is why `complaint` is read before the success shape, and why Phase 3
/// exists.
#[tokio::test]
#[ignore]
async fn a_failed_migration_blocks_the_next_apply_with_an_instruction() {
    reset("flyway_qa");
    let _ = flywaycli::run(&program(), &project(), "qa", "migrate", vec![]).await;

    let again = flywaycli::run(&program(), &project(), "qa", "migrate", vec![])
        .await
        .expect("flyway ran");

    let c = flywaycli::complaint(&again.stdout).expect("refused");
    assert_eq!(c.code.as_deref(), Some("VALIDATE_ERROR"));
    assert!(c.message.contains("run repair"), "{}", c.message);
    // The refusal carries nothing else at all, which is the trap this ordering
    // avoids: reading the success shape first would report an empty run.
    assert!(flywaycli::migrations(&again.stdout).is_err());
}

// -------------------------------------------------------------- repairing

/// Run SQL against a fixture database and return the output, so a test can see
/// what Flyway did to a schema rather than what Flyway says it did.
fn sql(db: &str, statement: &str) -> String {
    let out = std::process::Command::new("podman")
        .args([
            "exec",
            "-i",
            "db-query-mysql",
            "mysql",
            "-uroot",
            "-pdevpassword",
            "-N",
            db,
            "-e",
        ])
        .arg(statement)
        .output()
        .expect("podman exec");
    assert!(
        out.status.success(),
        "{statement}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// **F6, and the claim the confirmation makes.**
///
/// Repair rewrites the schema history and leaves the database alone. That
/// second half is the part a user is being asked to agree to, so it is the
/// part worth proving with `SHOW COLUMNS` rather than with Flyway's own
/// report of itself.
#[tokio::test]
#[ignore]
async fn repair_clears_the_failed_entry_and_changes_nothing_in_the_database() {
    reset("flyway_qa");
    let _ = flywaycli::run(&program(), &project(), "qa", "migrate", vec![]).await;

    // V1–V3 applied, V4 failed. That is the situation repair exists for.
    let before = sql("flyway_qa", "SHOW COLUMNS FROM widgets;");
    assert!(before.contains("colour"), "V3 ran: {before}");
    let history = sql(
        "flyway_qa",
        "SELECT version, success FROM flyway_schema_history ORDER BY installed_rank;",
    );
    assert!(
        history.contains("4\t0"),
        "V4 is recorded as failed: {history}"
    );

    let out = flywaycli::run(&program(), &project(), "qa", "repair", vec![])
        .await
        .expect("flyway ran");
    assert!(out.ok(), "{}", out.stderr);

    let r = flywaycli::repaired(&out.stdout).expect("a repair report");
    assert_eq!(r.removed.len(), 1);
    assert_eq!(r.removed[0].version.as_deref(), Some("4"));
    assert!(
        r.actions.iter().any(|a| a.contains("Removed failed")),
        "{:?}",
        r.actions
    );

    // The history lost exactly one row.
    let after = sql(
        "flyway_qa",
        "SELECT version, success FROM flyway_schema_history ORDER BY installed_rank;",
    );
    assert!(!after.contains("4\t0"), "the failed row is gone: {after}");
    assert!(
        after.contains("3\t1"),
        "and the successful ones are not: {after}"
    );

    // **And the schema is untouched.** This is what "it does not undo
    // anything" means, and it is the sentence the confirmation stakes itself on.
    assert_eq!(
        sql("flyway_qa", "SHOW COLUMNS FROM widgets;"),
        before,
        "repair must not have altered the table"
    );

    // Which is why V4 is Pending again rather than gone: the next apply will
    // run it from the start.
    let listed = flywaycli::migrations(&info("qa").await.stdout).unwrap();
    let v4 = listed
        .iter()
        .find(|m| m.version.as_deref() == Some("4"))
        .expect("V4 is still listed");
    assert_eq!(v4.group, flywaycli::Group::Pending);
}

/// Repairing a history with nothing wrong is a success that did nothing, and
/// the two must not be reported as the same thing.
#[tokio::test]
#[ignore]
async fn repairing_a_healthy_history_reports_that_it_did_nothing() {
    reset("flyway_qa");

    let out = flywaycli::run(&program(), &project(), "qa", "repair", vec![])
        .await
        .expect("flyway ran");
    assert!(out.ok());

    let r = flywaycli::repaired(&out.stdout).unwrap();
    assert!(r.is_empty(), "{r:?}");
    assert!(r.actions.is_empty(), "{:?}", r.actions);
}

/// **The case `info` cannot see.**
///
/// A migration edited after it ran keeps its `Success` state — nothing in the
/// list changes — and the only sign is `migrate` refusing with a checksum
/// mismatch. That is why `Complaint::suggests_repair` reads Flyway's message
/// rather than the migration's state, and why the Repair button can appear
/// when nothing is `Failed`.
///
/// Runs against a **copy** of the fixture, so the real one is never edited.
#[tokio::test]
#[ignore]
async fn an_edited_migration_looks_healthy_until_apply_and_repair_is_the_way_out() {
    use std::fs;

    reset("flyway_qa");
    let copy = std::env::temp_dir().join(format!(
        "db-query-drift-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(copy.join("migrations")).unwrap();
    fs::copy(project(), copy.join("flyway.toml")).unwrap();
    for entry in fs::read_dir(repo().join("dev/flyway/migrations")).unwrap() {
        let from = entry.unwrap().path();
        fs::copy(
            &from,
            copy.join("migrations").join(from.file_name().unwrap()),
        )
        .unwrap();
    }
    let toml = copy.join("flyway.toml");

    // Apply the three that work, leaving a clean history.
    let first = flywaycli::run(
        &program(),
        &toml,
        "qa",
        "migrate",
        vec!["-target=3".to_string()],
    )
    .await
    .expect("flyway ran");
    assert!(first.ok(), "{}", first.stderr);

    let before_v2 = sql(
        "flyway_qa",
        "SELECT version, installed_on, execution_time FROM flyway_schema_history \
         WHERE version = '2';",
    );

    assert!(
        before_v2.contains('2'),
        "the comparison below would be empty against empty: {before_v2}"
    );

    // Edit one of them, the way somebody does when they "just fix a typo".
    let v2 = copy.join("migrations/V2__seed_widgets.sql");
    let mut text = fs::read_to_string(&v2).unwrap();
    text.push_str("\n-- edited after it ran\n");
    fs::write(&v2, text).unwrap();

    // `info` notices nothing at all.
    let listed = flywaycli::migrations(
        &flywaycli::run(&program(), &toml, "qa", "info", vec![])
            .await
            .unwrap()
            .stdout,
    )
    .unwrap();
    let v2row = listed
        .iter()
        .find(|m| m.version.as_deref() == Some("2"))
        .unwrap();
    assert_eq!(v2row.state, "Success", "the list still looks healthy");
    assert_eq!(v2row.group, flywaycli::Group::Done);
    assert!(
        listed.iter().all(|m| m.group != flywaycli::Group::Failed),
        "nothing is failed, so nothing would reveal the Repair button"
    );

    // And the apply is refused, with the instruction the button now follows.
    let refused = flywaycli::run(&program(), &toml, "qa", "migrate", vec![])
        .await
        .unwrap();
    let c = flywaycli::complaint(&refused.stdout).expect("refused");
    assert_eq!(c.code.as_deref(), Some("VALIDATE_ERROR"));
    assert!(c.message.contains("checksum mismatch"), "{}", c.message);
    assert!(c.suggests_repair(), "{}", c.message);

    // **And repair is the way out, which is the other half of the claim.**
    // Measured against Flyway 13.5.0 on 2026-09-16: `repairActions` came back
    // as "Aligned applied migration checksums", and nothing was removed
    // — there was nothing failed to remove.
    let fixed = flywaycli::run(&program(), &toml, "qa", "repair", vec![])
        .await
        .expect("flyway ran");
    assert!(fixed.ok(), "{}", fixed.stderr);
    let r = flywaycli::repaired(&fixed.stdout).unwrap();
    assert_eq!(r.aligned.len(), 1, "{r:?}");
    assert_eq!(r.aligned[0].version.as_deref(), Some("2"));
    assert!(
        r.removed.is_empty(),
        "nothing failed, so nothing is removed"
    );
    assert!(r.deleted.is_empty(), "the file is still there");

    // **Nothing re-ran.** V2 keeps the timestamp and the execution time it was
    // first applied with: only its checksum column was rewritten. This is what
    // the confirmation means by "the edit is recorded as though it had always
    // been there, not applied to the database" — and it is the part somebody
    // pressing Repair is most likely to misread.
    let rows = sql(
        "flyway_qa",
        "SELECT version, installed_on, execution_time FROM flyway_schema_history \
         WHERE version = '2';",
    );
    assert_eq!(rows.trim(), before_v2.trim(), "V2 was re-run: {rows}");

    // And the apply that was refused now goes ahead.
    let after = flywaycli::run(
        &program(),
        &toml,
        "qa",
        "migrate",
        vec!["-target=3".to_string()],
    )
    .await
    .expect("flyway ran");
    assert!(after.ok(), "{}", after.stderr);
    assert!(
        flywaycli::complaint(&after.stdout).is_none(),
        "still refused: {}",
        after.stdout
    );

    let _ = fs::remove_dir_all(&copy);
}
