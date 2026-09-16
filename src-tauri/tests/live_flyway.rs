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
