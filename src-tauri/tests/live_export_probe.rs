//! Measurements for Stage 22 §9.4 and §9.5, against the live fixture.
//!
//! These are **measurements, not assertions about the app** — they answer the
//! two questions the export design branches on and that nothing short of a
//! real server can answer. They need the `export_probe` database built by the
//! commands in `docs/stage-22-schema-export.md`.
//!
//!     cargo test --test live_export_probe -- --ignored --nocapture --test-threads=1

use db_query_lib::schema::{self, ObjectDdl};
use db_query_lib::session::{self, AppState, ConnProfile};
use sqlx::{Connection, Executor, MySqlConnection, Row};
use std::time::Instant;

const URL: &str = "mysql://root:devpassword@127.0.0.1:3306/export_probe";

async fn connect() -> MySqlConnection {
    MySqlConnection::connect(URL)
        .await
        .expect("connect failed — is the fixture up and export_probe built?")
}

async fn count(conn: &mut MySqlConnection, sql: &str) -> i64 {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
}

/// **§9.4 — does a consistent snapshot hold across a long dump?**
///
/// The export reads dozens of tables over minutes while the app's own tabs
/// keep running statements. Without a snapshot it can contain a child row
/// whose parent it already passed.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn a_consistent_snapshot_holds_while_another_connection_writes() {
    let mut dump = connect().await;
    let mut other = connect().await;

    other
        .execute("DELETE FROM export_probe.audit")
        .await
        .unwrap();

    // Exactly what mysqldump --single-transaction does.
    dump.execute("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .unwrap();
    dump.execute("START TRANSACTION WITH CONSISTENT SNAPSHOT")
        .await
        .unwrap();

    let before = count(&mut dump, "SELECT COUNT(*) FROM audit").await;

    // Another connection writes, and commits, while the dump is open.
    for i in 0..5 {
        other
            .execute(
                sqlx::query("INSERT INTO audit (what) VALUES (?)")
                    .bind(format!("written during the dump {i}")),
            )
            .await
            .unwrap();
    }
    let outside = count(&mut other, "SELECT COUNT(*) FROM audit").await;

    // Read again, later, and through a *second* statement — the snapshot has
    // to span statements, not just one query.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let inside = count(&mut dump, "SELECT COUNT(*) FROM audit").await;
    let inside_again = count(&mut dump, "SELECT COUNT(*) FROM audit").await;

    println!(
        "snapshot: before={before} inside={inside} inside_again={inside_again} \
         outside={outside}"
    );
    assert_eq!(inside, before, "the snapshot moved");
    assert_eq!(
        inside_again, before,
        "the snapshot moved between statements"
    );
    assert_eq!(
        outside,
        before + 5,
        "the other connection did not actually write"
    );

    dump.execute("COMMIT").await.unwrap();
    // And once committed, the same connection sees the new world.
    assert_eq!(
        count(&mut dump, "SELECT COUNT(*) FROM audit").await,
        outside
    );
}

/// **§9.4b — can one connection hold the snapshot while reading many tables?**
///
/// DDL reads included: the dump interleaves `SHOW CREATE TABLE` with row
/// streaming, and a statement that implicitly commits would silently end the
/// snapshot half way through.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn reading_ddl_inside_the_snapshot_does_not_end_it() {
    let mut dump = connect().await;
    let mut other = connect().await;
    other
        .execute("DELETE FROM export_probe.audit")
        .await
        .unwrap();

    dump.execute("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .unwrap();
    dump.execute("START TRANSACTION WITH CONSISTENT SNAPSHOT")
        .await
        .unwrap();
    let before = count(&mut dump, "SELECT COUNT(*) FROM audit").await;

    // The things a dump does between tables.
    for sql in [
        "SHOW CREATE TABLE export_probe.t",
        "SHOW CREATE VIEW export_probe.v1",
        "SHOW TRIGGERS FROM export_probe",
    ] {
        let _ = sqlx::query(sql).fetch_all(&mut dump).await.unwrap();
    }
    // `USE` is the interesting one: it is how the script switches database.
    dump.execute("USE export_probe").await.unwrap();

    other
        .execute("INSERT INTO export_probe.audit (what) VALUES ('after the DDL reads')")
        .await
        .unwrap();

    let after = count(&mut dump, "SELECT COUNT(*) FROM audit").await;
    println!("before={before} after DDL reads and a USE={after}");
    assert_eq!(after, before, "something in there ended the snapshot");
}

/// **§9.5 — what batch size should `INSERT` use?**
///
/// Times a full stream of a million rows at each candidate, building the same
/// statements the export will write. The default should be a number that was
/// measured, not one that looked reasonable.
#[tokio::test]
#[ignore = "needs the live fixture, and reads a million rows several times"]
async fn measure_insert_batch_sizes() {
    use futures_util::StreamExt;

    for batch in [1usize, 100, 1_000, 10_000] {
        let mut conn = connect().await;
        let started = Instant::now();
        let mut rows_seen = 0usize;
        let mut bytes = 0u64;
        let mut statements = 0usize;
        let mut buf = String::new();

        let mut stream = sqlx::query("SELECT * FROM export_probe.wide").fetch(&mut conn);
        while let Some(row) = stream.next().await {
            let row = row.unwrap();
            // Stand-in for `export::insert_row`: the cost being measured is
            // the fetch and the string building, not the exact literal form.
            if rows_seen % batch == 0 {
                if !buf.is_empty() {
                    buf.push_str(";\n");
                    bytes += buf.len() as u64;
                    buf.clear();
                    statements += 1;
                }
                buf.push_str("INSERT INTO `wide` VALUES ");
            } else {
                buf.push(',');
            }
            let id: i64 = row.try_get(0).unwrap();
            let name: String = row.try_get(1).unwrap();
            buf.push_str(&format!("({id},'{name}')"));
            rows_seen += 1;
        }
        if !buf.is_empty() {
            bytes += buf.len() as u64;
            statements += 1;
        }
        let took = started.elapsed();
        println!(
            "batch {batch:>6}: {rows_seen} rows in {took:?} \
             ({:.0} rows/s), {statements} statements, {:.1} MB",
            rows_seen as f64 / took.as_secs_f64(),
            bytes as f64 / 1_048_576.0
        );
    }
}

/// **§9.5b — what batching actually buys: load time.**
///
/// The read is the same speed whatever the batch (above), so the number that
/// decides the default is how long the generated script takes to *run*, which
/// is what somebody restoring a dump waits for. Writes three real scripts and
/// loads each into a scratch database.
///
/// Also records the longest statement produced, because `max_allowed_packet`
/// (64 MB by default) is a hard ceiling a large batch can walk into.
#[tokio::test]
#[ignore = "needs the live fixture; writes and loads three scripts"]
async fn measure_load_time_by_batch_size() {
    use futures_util::StreamExt;
    use std::io::Write;

    // A tenth of the table: enough to time, small enough to load three times.
    const ROWS: usize = 100_000;
    let dir = std::env::temp_dir().join("db-query-export-probe");
    std::fs::create_dir_all(&dir).unwrap();

    for batch in [1usize, 100, 1_000, 10_000] {
        let mut conn = connect().await;
        let path = dir.join(format!("wide-{batch}.sql"));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "SET FOREIGN_KEY_CHECKS=0;").unwrap();
        writeln!(file, "DROP TABLE IF EXISTS load_target;").unwrap();
        writeln!(file, "CREATE TABLE load_target LIKE export_probe.wide;").unwrap();

        let mut buf = String::new();
        let mut seen = 0usize;
        let mut longest = 0usize;
        let mut stream = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT id, name, email, amount FROM export_probe.wide LIMIT {ROWS}"
        )))
        .fetch(&mut conn);
        while let Some(row) = stream.next().await {
            let row = row.unwrap();
            if seen % batch == 0 {
                if !buf.is_empty() {
                    buf.push_str(";\n");
                    longest = longest.max(buf.len());
                    file.write_all(buf.as_bytes()).unwrap();
                    buf.clear();
                }
                buf.push_str("INSERT INTO load_target (id, name, email, amount) VALUES ");
            } else {
                buf.push(',');
            }
            let id: i64 = row.try_get(0).unwrap();
            let name: String = row.try_get(1).unwrap();
            let email: String = row.try_get(2).unwrap();
            // As a string: it is what lands in the file either way, and the
            // measurement is of bytes and load time, not of decimal decoding.
            let amount: String = row.try_get::<String, _>(3).unwrap_or_else(|_| "0".into());
            buf.push_str(&format!("({id},'{name}','{email}',{amount})"));
            seen += 1;
        }
        if !buf.is_empty() {
            buf.push_str(";\n");
            longest = longest.max(buf.len());
            file.write_all(buf.as_bytes()).unwrap();
        }
        drop(file);

        let bytes = std::fs::metadata(&path).unwrap().len();
        let started = Instant::now();
        let out = std::process::Command::new("podman")
            .args([
                "exec",
                "-i",
                "db-query-mysql",
                "mysql",
                "-uroot",
                "-pdevpassword",
                "export_probe",
            ])
            .stdin(std::fs::File::open(&path).unwrap())
            .output()
            .expect("podman exec failed");
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "load failed for batch {batch}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        println!(
            "batch {batch:>6}: {ROWS} rows, {:.1} MB, longest statement {:.2} MB, \
             loaded in {took:?}",
            bytes as f64 / 1_048_576.0,
            longest as f64 / 1_048_576.0,
        );
    }
}

// ------------------------------- the introspection, against the real server

const C: &str = "c-export";

fn probe_profile() -> ConnProfile {
    ConnProfile {
        id: C.into(),
        name: C.into(),
        colour: "#3b82f6".into(),
        host: "127.0.0.1".into(),
        port: 3306,
        user: "root".into(),
        database: Some("export_probe".into()),
        allow_invalid_certs: true,
        kind: Default::default(),
        url: String::new(),
        auth: Default::default(),
        no_password: false,
        read_only: false,
    }
}

async fn connected() -> AppState {
    let state = AppState::default();
    session::connect(&state, probe_profile(), "devpassword".into())
        .await
        .expect("connect failed — `mise run db-up && mise run export-probe-up`?");
    state
}

/// **The ordering that is recorded nowhere else.**
///
/// The fixture has two `BEFORE INSERT` triggers on `t`. Their relative order
/// exists only in `ACTION_ORDER`, and `SHOW CREATE TRIGGER` emits no
/// `FOLLOWS`/`PRECEDES` to carry it — so this is the one place the app can
/// learn it, and the unit tests in `dump` are only as good as this is.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn triggers_come_back_with_the_order_only_the_server_knows() {
    let state = connected().await;
    let found = schema::triggers(&state, C, "export_probe").await.unwrap();

    let on_t: Vec<_> = found
        .iter()
        .filter(|t| t.table == "t" && t.timing == "BEFORE" && t.event == "INSERT")
        .collect();
    assert_eq!(on_t.len(), 2, "the fixture has two: {found:#?}");
    let ordered = db_query_lib::dump::order_triggers(&found);
    let names: Vec<&str> = ordered
        .iter()
        .filter(|t| t.table == "t" && t.timing == "BEFORE")
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, ["t_before_ins", "t_before_ins2"]);
    assert_eq!(on_t.iter().map(|t| t.action_order).min(), Some(1));
    assert_eq!(on_t.iter().map(|t| t.action_order).max(), Some(2));
}

/// And its text round-trips, with the `sql_mode` it was written under.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn a_trigger_definition_comes_back_whole() {
    let state = connected().await;
    let def = schema::object_ddl(&state, C, "export_probe", "t_after_ins", ObjectDdl::Trigger)
        .await
        .unwrap();

    assert!(def.body.contains("TRIGGER"), "{}", def.body);
    assert!(def.body.contains("t_after_ins"), "{}", def.body);
    // The body names `audit` unqualified, which is why the script must `USE`
    // before creating triggers.
    assert!(def.body.contains("audit"), "{}", def.body);
    assert!(
        !def.sql_mode.is_empty(),
        "the sql_mode is what makes it mean the same"
    );
    assert_eq!(def.time_zone, None, "triggers have no time zone; events do");

    // And the definer is there to be stripped, which is the only reason the
    // stripping exists.
    assert!(def.body.contains("DEFINER="), "{}", def.body);
    assert!(!db_query_lib::dump::strip_definer(&def.body).contains("DEFINER="));
}

/// An event carries a time zone, because its `STARTS` is an absolute timestamp
/// read in one.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn an_event_comes_back_with_its_time_zone() {
    let state = connected().await;
    let found = schema::events(&state, C, "export_probe").await.unwrap();
    assert_eq!(
        found.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["e_daily"]
    );

    let def = schema::object_ddl(&state, C, "export_probe", "e_daily", ObjectDdl::Event)
        .await
        .unwrap();
    assert!(def.body.contains("EVENT"), "{}", def.body);
    assert!(
        def.body.contains("STARTS"),
        "an absolute time: {}",
        def.body
    );
    assert!(def.time_zone.is_some(), "which is read in a time zone");
}

/// **§9.1 against the server rather than against the note that records it.**
///
/// The fixture's chain is `v3 → v2 → v1 → t`. Only the view-to-view edges come
/// back, and sorting by them produces an order that can actually be run.
#[tokio::test]
#[ignore = "needs the live fixture"]
async fn views_sort_into_an_order_that_can_be_created() {
    let state = connected().await;
    let edges = schema::view_dependencies(&state, C, "export_probe")
        .await
        .unwrap();

    // v1 reads only a base table, so it has no edge here at all.
    assert!(
        edges.contains(&("v2".to_string(), "v1".to_string())),
        "{edges:?}"
    );
    assert!(
        edges.contains(&("v3".to_string(), "v2".to_string())),
        "{edges:?}"
    );
    assert!(
        !edges.iter().any(|(_, viewed)| viewed == "t"),
        "base tables are not constraints: {edges:?}"
    );

    let order =
        db_query_lib::dump::order_views(&["v3".into(), "v2".into(), "v1".into()], &edges).unwrap();
    assert_eq!(order, ["v1", "v2", "v3"]);
}

// ----------------------------------------------- the round trip, end to end

/// Load a script into a database, through the client, and say what failed.
fn load(script: &std::path::Path, into: &str) -> Result<(), String> {
    let out = std::process::Command::new("podman")
        .args([
            "exec",
            "-i",
            "db-query-mysql",
            "mysql",
            "-uroot",
            "-pdevpassword",
            into,
        ])
        .stdin(std::fs::File::open(script).map_err(|e| e.to_string())?)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).to_string())
}

async fn scratch(name: &str) -> MySqlConnection {
    let mut root = MySqlConnection::connect("mysql://root:devpassword@127.0.0.1:3306")
        .await
        .unwrap();
    root.execute(sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS `{name}`"
    ))))
    .await
    .unwrap();
    root.execute(sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE `{name}`"
    ))))
    .await
    .unwrap();
    root
}

/// **The claim the whole feature serves: the script re-runs.**
///
/// Everything else here — the ordering, the header, the DEFINER stripping — is
/// in service of this, and nothing short of loading the file proves it. So:
/// dump `export_probe` into a file, load that file into an empty database, and
/// compare what arrives against what was there.
///
/// Deliberately the probe database rather than `poc`: it is the one with a
/// view on a view, two triggers whose order is recorded nowhere but
/// `ACTION_ORDER`, and an event.
#[tokio::test]
#[ignore = "needs the live fixture; writes a file and loads it"]
async fn a_dumped_schema_loads_into_an_empty_database() {
    use db_query_lib::dump::{Contents, DumpOptions};
    use db_query_lib::dumprun::{DbPick, Job, Layout};

    const TARGET: &str = "export_roundtrip";
    let mut root = scratch(TARGET).await;

    let dir = std::env::temp_dir().join("db-query-roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("probe.sql");

    let mut conn = connect().await;
    let picks = [DbPick {
        db: "export_probe".into(),
        // Everything, resolved as it runs — which is what ticking the
        // database in the tree means.
        tables: Some(vec!["t".into(), "audit".into()]),
        views: None,
        routines: None,
        triggers: None,
        events: None,
    }];
    let job = Job {
        picks: &picks,
        options: DumpOptions {
            contents: Contents::SchemaAndData,
            // The script names no database and is loaded into one chosen on
            // the command line, which is also how somebody restores into a
            // differently-named database.
            create_database: false,
            ..DumpOptions::default()
        },
        layout: Layout::SingleFile,
        target: out.clone(),
    };

    let outcome = db_query_lib::dumprun::run(&mut conn, &job, |_| {}, || false)
        .await
        .expect("the dump should run");
    println!(
        "wrote {} bytes, {} tables, {} rows",
        outcome.bytes_written, outcome.tables, outcome.rows
    );
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(!outcome.cancelled);

    // The part file is gone and the real one is there.
    assert!(out.exists(), "the script was not renamed into place");
    assert!(
        !dir.join("probe.sql.part").exists(),
        "a part file was left behind"
    );

    load(&out, TARGET).unwrap_or_else(|e| {
        panic!(
            "the script did not load:\n{e}\n--- script ---\n{}",
            std::fs::read_to_string(&out).unwrap()
        )
    });

    // --- and now: is what arrived the same thing?

    let names = |rows: Vec<sqlx::mysql::MySqlRow>| -> Vec<String> {
        rows.into_iter().map(|r| r.try_get(0).unwrap()).collect()
    };

    let tables = names(
        sqlx::query(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
        )
        .bind(TARGET)
        .fetch_all(&mut root)
        .await
        .unwrap(),
    );
    assert_eq!(tables, ["audit", "t"]);

    // **The view chain.** v2 reads v1 and v3 reads v2, so if they were written
    // in the order the server lists them the load above would already have
    // failed — this says they all arrived.
    let views = names(
        sqlx::query(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'VIEW' ORDER BY TABLE_NAME",
        )
        .bind(TARGET)
        .fetch_all(&mut root)
        .await
        .unwrap(),
    );
    assert_eq!(views, ["v1", "v2", "v3"]);

    // **The triggers, in the order only ACTION_ORDER records.**
    let triggers: Vec<(String, u32)> = sqlx::query(
        "SELECT TRIGGER_NAME, ACTION_ORDER FROM information_schema.TRIGGERS \
         WHERE TRIGGER_SCHEMA = ? AND ACTION_TIMING = 'BEFORE' \
         ORDER BY ACTION_ORDER",
    )
    .bind(TARGET)
    .fetch_all(&mut root)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.try_get(0).unwrap(), r.try_get(1).unwrap()))
    .collect();
    assert_eq!(
        triggers,
        [
            ("t_before_ins".to_string(), 1),
            ("t_before_ins2".to_string(), 2)
        ],
        "the two BEFORE INSERT triggers came back in the wrong order"
    );

    let events = names(
        sqlx::query("SELECT EVENT_NAME FROM information_schema.EVENTS WHERE EVENT_SCHEMA = ?")
            .bind(TARGET)
            .fetch_all(&mut root)
            .await
            .unwrap(),
    );
    assert_eq!(events, ["e_daily"]);

    // And the rows. `t` is empty in the fixture, so this is mostly about the
    // data path running at all; `measure_insert_batch_sizes` covers volume.
    let rows: i64 = count(&mut root, &format!("SELECT COUNT(*) FROM `{TARGET}`.t")).await;
    let source_rows = count(&mut conn, "SELECT COUNT(*) FROM export_probe.t").await;
    assert_eq!(rows, source_rows);
}

/// **Data, at a volume where the batching matters.**
///
/// A thousand rows is more than one batch at the default, so this also proves
/// a statement is closed and a new one opened in the middle of a table.
#[tokio::test]
#[ignore = "needs the live fixture; writes a file and loads it"]
async fn dumped_rows_arrive_in_the_same_number_and_shape() {
    use db_query_lib::dump::DumpOptions;
    use db_query_lib::dumprun::{DbPick, Job, Layout};

    const TARGET: &str = "export_roundtrip_rows";
    let mut root = scratch(TARGET).await;

    // A slice of `wide`, which has a DECIMAL, a TEXT, a DATETIME and a binary
    // column — the four that a careless literal gets wrong.
    let mut conn = connect().await;
    conn.execute("DROP TABLE IF EXISTS export_probe.slice")
        .await
        .unwrap();
    conn.execute("CREATE TABLE export_probe.slice AS SELECT * FROM export_probe.wide LIMIT 2500")
        .await
        .unwrap();

    let dir = std::env::temp_dir().join("db-query-roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("rows.sql");

    let picks = [DbPick {
        db: "export_probe".into(),
        tables: Some(vec!["slice".into()]),
        views: Some(vec![]),
        routines: Some(vec![]),
        triggers: Some(vec![]),
        events: Some(vec![]),
    }];
    let job = Job {
        picks: &picks,
        options: DumpOptions {
            create_database: false,
            ..DumpOptions::default()
        },
        layout: Layout::SingleFile,
        target: out.clone(),
    };

    let outcome = db_query_lib::dumprun::run(&mut conn, &job, |_| {}, || false)
        .await
        .unwrap();
    assert_eq!(outcome.rows, 2500);

    load(&out, TARGET).unwrap_or_else(|e| panic!("did not load: {e}"));

    assert_eq!(
        count(&mut root, &format!("SELECT COUNT(*) FROM `{TARGET}`.slice")).await,
        2500
    );
    // Not just the count: a checksum over every column, so a mangled DECIMAL
    // or a dropped binary byte fails here rather than passing as "2500 rows".
    let source: Vec<String> = sqlx::query("CHECKSUM TABLE export_probe.slice")
        .fetch_all(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.try_get::<i64, _>(1).unwrap().to_string())
        .collect();
    let loaded: Vec<String> = sqlx::query(sqlx::AssertSqlSafe(format!(
        "CHECKSUM TABLE `{TARGET}`.slice"
    )))
    .fetch_all(&mut root)
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.try_get::<i64, _>(1).unwrap().to_string())
    .collect();
    assert_eq!(source, loaded, "the rows are not byte-for-byte the same");
}
