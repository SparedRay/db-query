//! The batch runner against the real fixture folder and a real server.
//!
//!     mise run db-up
//!     cargo test --test live_sqlfiles -- --ignored --test-threads=1 --nocapture
//!
//! `dev/scripts` holds each case on purpose (see its README), and
//! `01-setup.sql` / `99-teardown.sql` make the folder re-runnable, so these
//! tests leave `poc` as they found it.
//!
//! Single-threaded: they share one database and one fixture table.

use db_query_lib::engine::Capabilities;
use db_query_lib::export::CsvOptions;
use db_query_lib::sqlfiles;
use db_query_lib::sqlfilesrun::{self, FileStatus, Job, Progress};
use sqlx::{Connection, MySqlConnection};
use std::path::PathBuf;

const URL: &str = "mysql://root:devpassword@127.0.0.1:3306/poc";

async fn connect() -> MySqlConnection {
    MySqlConnection::connect(URL)
        .await
        .expect("connect failed — is the fixture up? `mise run db-up`")
}

fn scripts() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("dev/scripts")
}

fn out_dir(tag: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("db-query-sqlfiles-{tag}-{stamp}"));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn csv_opts() -> CsvOptions {
    CsvOptions {
        delimiter: ",".into(),
        crlf: false,
        bom: false,
        headers: true,
        null_as: String::new(),
        formula_guard: false,
    }
}

fn status_of<'a>(out: &'a sqlfilesrun::RunOutcome, path: &str) -> &'a FileStatus {
    &out.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no {path} in the report"))
        .status
}

/// **The whole folder, end to end.** The claim the feature exists for: run
/// every script, write every result set, and report what happened to each file.
#[tokio::test]
#[ignore = "needs the live fixture; writes files"]
async fn the_fixture_folder_runs_and_writes_its_csvs() {
    let source = scripts();
    let dest = out_dir("run");
    let scan = sqlfiles::read_and_scan(&source, true).unwrap();
    let caps = Capabilities::mysql();
    let job = Job {
        scan: &scan,
        source: source.clone(),
        dest: dest.clone(),
        database: Some("poc".into()),
        csv: csv_opts(),
        caps: &caps,
    };

    let mut seen_planned = None;
    let mut statements_reported = 0usize;
    let out = sqlfilesrun::run(
        &mut connect().await,
        &job,
        |p| match p {
            Progress::Planned { statements, .. } => seen_planned = Some(statements),
            Progress::Statement { .. } => statements_reported += 1,
            _ => {}
        },
        || false,
    )
    .await
    .expect("the run should finish");

    // Progress described the plan before anything ran, and every statement
    // announced itself.
    assert_eq!(seen_planned, Some(12));
    assert_eq!(statements_reported, 12);

    assert!(!out.cancelled);
    assert_eq!(out.stopped_at, None, "nothing should have failed");
    assert_eq!(out.csvs, 7);

    // Every file is in the report, including the two that never run.
    assert_eq!(out.files.len(), 9);
    assert!(matches!(status_of(&out, "01-setup.sql"), FileStatus::Ran));
    assert!(matches!(
        status_of(&out, "04-proc.sql"),
        FileStatus::Skipped { .. }
    ));
    assert!(matches!(
        status_of(&out, "05-notes.sql"),
        FileStatus::Skipped { .. }
    ));

    // **The CSVs are on disk, under the names the plan promised**, with the
    // subfolder mirrored rather than flattened.
    for name in [
        "02-orders.csv",
        "03-report.csv",
        "03-report-2.csv",
        "03-report-3.csv",
        "06-no-semicolon.csv",
        "07-bom.csv",
        "daily/08-nested.csv",
    ] {
        let p = sqlfiles::under(&dest, name);
        assert!(p.is_file(), "{name} was not written");
        assert!(
            !p.with_extension("csv.part").exists(),
            "{name} left a part file behind"
        );
    }

    // And the content is the result, not a placeholder: three rows and a
    // header, with the row that has multi-byte text intact.
    let orders = std::fs::read_to_string(dest.join("02-orders.csv")).unwrap();
    let lines: Vec<&str> = orders.lines().collect();
    assert_eq!(lines[0], "id,label,amount");
    assert_eq!(lines.len(), 4, "{orders}");
    assert!(orders.contains("héllo ★"), "{orders}");
    // DECIMAL arrives as text, so it must not have been through an f64.
    assert!(orders.contains("10.50"), "{orders}");

    // **The BOM'd script ran.** Without the strip this is MySQL error 1064
    // pointing at a character nobody can see (§9.7).
    let bom = std::fs::read_to_string(dest.join("07-bom.csv")).unwrap();
    assert_eq!(bom.lines().collect::<Vec<_>>(), ["amount", "0.01"]);

    // The writes are counted where they happened.
    let setup = out.files.iter().find(|f| f.path == "01-setup.sql").unwrap();
    assert_eq!(setup.statements_run, 3);
    assert_eq!(setup.rows_affected, 3, "the INSERT's three rows");

    // **The nested script ran before the teardown.** Files run in path order,
    // so `daily/…` sorts after every top-level file — which is why the
    // teardown is `zz-` and not `99-`. It is asserted rather than only fixed:
    // with the old name the nested file failed, and a reader of this test
    // should see why the name matters.
    let order: Vec<&str> = out.files.iter().map(|f| f.path.as_str()).collect();
    let nested = order
        .iter()
        .position(|p| *p == "daily/08-nested.sql")
        .unwrap();
    let teardown = order.iter().position(|p| *p == "zz-teardown.sql").unwrap();
    assert!(nested < teardown, "{order:?}");
    assert!(matches!(
        status_of(&out, "daily/08-nested.sql"),
        FileStatus::Ran
    ));

    // The teardown ran too, so the fixture is as it was found.
    assert!(matches!(
        status_of(&out, "zz-teardown.sql"),
        FileStatus::Ran
    ));
    std::fs::remove_dir_all(&dest).ok();
}

/// **Stop-on-error, and the rows a report by omission would leave out.**
///
/// §4.3 promises only one thing: nothing after a failure runs. So a folder
/// whose second file is broken must leave the third one's CSV absent *and say
/// so* — five successes and one failure, silent about what was never
/// attempted, is a report that misleads.
#[tokio::test]
#[ignore = "needs the live fixture; writes files"]
async fn a_failure_stops_the_run_and_the_rest_is_reported_as_not_run() {
    let source = out_dir("stop-src");
    std::fs::write(source.join("1-ok.sql"), "SELECT 1 AS a;\n").unwrap();
    std::fs::write(
        source.join("2-bad.sql"),
        "-- a note\nSELECT * FROM no_such_table;\n",
    )
    .unwrap();
    std::fs::write(source.join("3-never.sql"), "SELECT 3 AS c;\n").unwrap();
    let dest = out_dir("stop-out");

    let scan = sqlfiles::read_and_scan(&source, false).unwrap();
    let caps = Capabilities::mysql();
    let job = Job {
        scan: &scan,
        source: source.clone(),
        dest: dest.clone(),
        database: Some("poc".into()),
        csv: csv_opts(),
        caps: &caps,
    };
    let out = sqlfilesrun::run(&mut connect().await, &job, |_| {}, || false)
        .await
        .expect("a failing statement is a reported failure, not a failed run");

    assert!(matches!(status_of(&out, "1-ok.sql"), FileStatus::Ran));
    match status_of(&out, "2-bad.sql") {
        FileStatus::Failed {
            statement,
            line,
            message,
        } => {
            assert_eq!(*statement, 1);
            // **Line 2, not line 1** — the span owns the comment above it, and
            // somebody sent to line 1 would find the note, not the SELECT.
            assert_eq!(*line, 2);
            assert!(message.contains("no_such_table"), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(matches!(status_of(&out, "3-never.sql"), FileStatus::NotRun));
    assert_eq!(out.stopped_at.as_deref(), Some("2-bad.sql:2"));

    // The first file's CSV is there; the third's is not, and nothing was left
    // half-written.
    assert!(dest.join("1-ok.csv").is_file());
    assert!(!dest.join("3-never.csv").exists());
    assert!(!dest.join("2-bad.csv").exists());
    assert_eq!(out.csvs, 1);

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&dest).ok();
}

/// **A `USE` inside one script moves the next one**, and the report says where
/// each file actually ran (§3.4). §9.5 measured that it persists; this is the
/// consequence the report has to tell the truth about.
#[tokio::test]
#[ignore = "needs the live fixture; writes files"]
async fn a_use_in_one_script_is_reported_against_the_next() {
    let source = out_dir("use-src");
    std::fs::write(source.join("1-here.sql"), "SELECT DATABASE() AS db;\n").unwrap();
    std::fs::write(source.join("2-switch.sql"), "USE information_schema;\n").unwrap();
    std::fs::write(source.join("3-there.sql"), "SELECT DATABASE() AS db;\n").unwrap();
    let dest = out_dir("use-out");

    let scan = sqlfiles::read_and_scan(&source, false).unwrap();
    let caps = Capabilities::mysql();
    let job = Job {
        scan: &scan,
        source: source.clone(),
        dest: dest.clone(),
        database: Some("poc".into()),
        csv: csv_opts(),
        caps: &caps,
    };
    let out = sqlfilesrun::run(&mut connect().await, &job, |_| {}, || false)
        .await
        .unwrap();

    let db_of = |path: &str| {
        out.files
            .iter()
            .find(|f| f.path == path)
            .unwrap()
            .database
            .clone()
    };
    assert_eq!(db_of("1-here.sql").as_deref(), Some("poc"));
    // The file that did the switching is reported where it ended up, which is
    // also where everything after it will run.
    assert_eq!(db_of("2-switch.sql").as_deref(), Some("information_schema"));
    assert_eq!(db_of("3-there.sql").as_deref(), Some("information_schema"));

    // And the CSVs agree with the report, which is the point: the second one
    // really did run somewhere else.
    let first = std::fs::read_to_string(dest.join("1-here.csv")).unwrap();
    let third = std::fs::read_to_string(dest.join("3-there.csv")).unwrap();
    assert!(first.contains("poc"), "{first}");
    assert!(third.contains("information_schema"), "{third}");

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&dest).ok();
}

/// **A read-only connection refuses the write before it is sent**, and the
/// refusal names the real reason rather than relaying the server's.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn a_read_only_connection_refuses_a_writing_script_without_sending_it() {
    let source = out_dir("ro-src");
    std::fs::write(
        source.join("1-write.sql"),
        "INSERT INTO batch_probe VALUES (99, 'nope', 1.00);\n",
    )
    .unwrap();
    let dest = out_dir("ro-out");

    let scan = sqlfiles::read_and_scan(&source, false).unwrap();
    let mut caps = Capabilities::mysql();
    caps.writes = false;
    caps.read_only = true;
    let job = Job {
        scan: &scan,
        source: source.clone(),
        dest: dest.clone(),
        database: Some("poc".into()),
        csv: csv_opts(),
        caps: &caps,
    };
    let out = sqlfilesrun::run(&mut connect().await, &job, |_| {}, || false)
        .await
        .unwrap();

    match status_of(&out, "1-write.sql") {
        FileStatus::Failed { message, .. } => {
            assert!(message.contains("read-only"), "{message}");
            // The connection's own doing, not a claim about MySQL.
            assert!(!message.to_lowercase().contains("mysql is"), "{message}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    // Nothing reached the server: the table does not even exist here, so a
    // statement that was sent would have failed with a *different* message.
    assert_eq!(out.csvs, 0);

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&dest).ok();
}

/// **Stop lands inside a result, and keeps what it wrote.**
///
/// §9.3 measured that an abandoned stream leaves the connection usable, which
/// is what makes this safe. The CSV it was writing is kept and **marked
/// partial** — a silently truncated CSV is one somebody opens in a spreadsheet
/// and believes.
#[tokio::test]
#[ignore = "needs the live fixture; writes files"]
async fn stopping_mid_result_keeps_the_rows_it_had_and_says_they_are_partial() {
    let source = out_dir("stop-mid-src");
    // `big` has 7500 rows, so there is time to change our mind.
    std::fs::write(source.join("1-big.sql"), "SELECT * FROM big;\n").unwrap();
    std::fs::write(source.join("2-after.sql"), "SELECT 1 AS a;\n").unwrap();
    let dest = out_dir("stop-mid-out");

    let scan = sqlfiles::read_and_scan(&source, false).unwrap();
    let caps = Capabilities::mysql();
    let job = Job {
        scan: &scan,
        source: source.clone(),
        dest: dest.clone(),
        database: Some("poc".into()),
        csv: csv_opts(),
        caps: &caps,
    };

    // Cancel as soon as the first statement is announced, which is before its
    // first row has arrived.
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let setter = flag.clone();
    let out = sqlfilesrun::run(
        &mut connect().await,
        &job,
        move |p| {
            if matches!(p, Progress::Statement { .. }) {
                setter.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        },
        || flag.load(std::sync::atomic::Ordering::SeqCst),
    )
    .await
    .expect("a cancelled run still returns an outcome");

    assert!(out.cancelled);
    let first = out.files.iter().find(|f| f.path == "1-big.sql").unwrap();
    assert_eq!(first.csvs.len(), 1);
    assert!(
        first.csvs[0].partial,
        "a CSV cut short must say so: {:?}",
        first.csvs[0]
    );
    // It kept what it had, and the file is real rather than a `.part`.
    let p = dest.join("1-big.csv");
    assert!(p.is_file());
    assert!(!dest.join("1-big.csv.part").exists());

    // And the file after it was not attempted.
    assert!(matches!(status_of(&out, "2-after.sql"), FileStatus::NotRun));

    std::fs::remove_dir_all(&source).ok();
    std::fs::remove_dir_all(&dest).ok();
}
