//! Running a schema export: one connection, one snapshot, many objects.
//!
//! [`crate::dump`] decides what the file says. This decides when, reads the
//! server, and writes the bytes.
//!
//! Three things make it its own module rather than another arm of
//! `exec::stream_to_file`:
//!
//!   * **It owns a connection for its whole run.** `stream_to_file` borrows a
//!     tab's, which would freeze somebody's editor for the length of a dump —
//!     and the consistent snapshot (§4.5) has to span every table anyway.
//!   * **One file, many producers.** That function owns a file end to end;
//!     here DDL, rows and more DDL append to the same sink.
//!   * **It batches while streaming.** `export::insert_row` deliberately does
//!     not — see §9.5 for what that costs on load.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sqlx::{Executor, MySqlConnection, Row};

use crate::decode::columns_of;
use crate::dump::{self, DumpOptions};
use crate::schema::{self, ObjectDdl};

/// What to export, per database.
///
/// `None` anywhere means **everything of that kind, resolved when the export
/// runs** — which is what ticking a database in the tree has to mean, since
/// the tree lazy-loads and must not fetch every table to record a tick
/// (§3.1).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbPick {
    pub db: String,
    pub tables: Option<Vec<String>>,
    pub views: Option<Vec<String>>,
    pub routines: Option<Vec<String>>,
    pub triggers: Option<Vec<String>>,
    pub events: Option<Vec<String>>,
}

/// One file, or one per database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    SingleFile,
    FilePerDatabase,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DumpOutcome {
    pub files: Vec<String>,
    pub bytes_written: u64,
    pub tables: usize,
    pub rows: u64,
    pub cancelled: bool,
    pub warnings: Vec<String>,
}

/// What the UI is told while it runs.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Progress {
    /// Everything that will be written, known once the selection is resolved.
    Planned {
        objects: usize,
    },
    /// Starting on one object.
    Object {
        kind: String,
        db: String,
        name: String,
        done: usize,
        total: usize,
    },
    /// Rows so far for the object in flight. Unlike Flyway, we are the ones
    /// doing the work, so this is a real count rather than a guess.
    Rows {
        rows: u64,
        bytes: u64,
    },
    Finished,
}

// ------------------------------------------------------------------- sink

/// A file being written, through a `.part` that is renamed at the end.
///
/// An export that was cancelled or that crashed must never leave a
/// half-written file wearing the real name — somebody would restore from it.
struct Sink {
    path: PathBuf,
    tmp: PathBuf,
    file: std::fs::File,
    bytes: u64,
    /// Set by [`Sink::finish`]. While it is false, dropping removes the part
    /// file — see the `Drop` impl below.
    finished: bool,
}

impl Sink {
    fn create(path: &Path) -> Result<Self, String> {
        let tmp = path.with_extension(format!(
            "{}.part",
            path.extension().and_then(|e| e.to_str()).unwrap_or("sql")
        ));
        let file = std::fs::File::create(&tmp)
            .map_err(|e| format!("Cannot create {}: {e}", tmp.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            tmp,
            file,
            bytes: 0,
            finished: false,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), String> {
        self.file
            .write_all(text.as_bytes())
            .map_err(|e| format!("Cannot write {}: {e}", self.tmp.display()))?;
        self.bytes += text.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> Result<u64, String> {
        self.file
            .flush()
            .map_err(|e| format!("Cannot flush {}: {e}", self.tmp.display()))?;
        std::fs::rename(&self.tmp, &self.path).map_err(|e| {
            format!(
                "Cannot rename {} to {}: {e}",
                self.tmp.display(),
                self.path.display()
            )
        })?;
        self.finished = true;
        Ok(self.bytes)
    }
}

/// **A part file never outlives the export that was writing it.**
///
/// Every failure in here leaves through `?`, and there are a dozen of them — a
/// disk filling up, a dropped connection, a name that cannot be quoted. A
/// tidy-up written at each one is a tidy-up that will be missed at the
/// thirteenth, so it lives where it cannot be: the only way to keep the file
/// is to have renamed it.
impl Drop for Sink {
    fn drop(&mut self) {
        if !self.finished {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

// ------------------------------------------------------------------- run

/// Everything the runner needs that is not a connection.
pub struct Job<'a> {
    pub picks: &'a [DbPick],
    pub options: DumpOptions,
    pub layout: Layout,
    /// A file for `SingleFile`, a directory for `FilePerDatabase`.
    pub target: PathBuf,
}

/// Run the export.
///
/// `report` is called as it goes; `cancelled` is polled between objects and
/// between batches, so a stop lands within one batch rather than at the end of
/// a table.
pub async fn run(
    conn: &mut MySqlConnection,
    job: &Job<'_>,
    mut report: impl FnMut(Progress),
    cancelled: impl Fn() -> bool,
) -> Result<DumpOutcome, String> {
    let mut warnings = Vec::new();

    // **The snapshot.** Exactly what `mysqldump --single-transaction` does,
    // and the reason this has its own connection: a dump taken over minutes,
    // while the app's own tabs commit, would otherwise contain a child row
    // whose parent it had already passed. Measured to survive `SHOW CREATE`
    // and `USE` — §9.4.
    conn.execute("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .map_err(|e| format!("Cannot set the isolation level: {e}"))?;
    conn.execute("START TRANSACTION WITH CONSISTENT SNAPSHOT")
        .await
        .map_err(|e| format!("Cannot open a consistent snapshot: {e}"))?;

    // Resolve the selection first, so the total is known before anything is
    // written and the progress bar is not a guess.
    let mut plans = Vec::new();
    for pick in job.picks {
        plans.push(resolve(conn, pick, &job.options, &mut warnings).await?);
    }
    let total: usize = plans.iter().map(Plan::objects).sum();
    report(Progress::Planned { objects: total });

    let mut files = Vec::new();
    let mut bytes_written = 0u64;
    let mut tables = 0usize;
    let mut rows_total = 0u64;
    let mut done = 0usize;
    let mut stopped = false;

    let mut sink = match job.layout {
        Layout::SingleFile => Some(Sink::create(&job.target)?),
        Layout::FilePerDatabase => None,
    };
    if let Some(s) = sink.as_mut() {
        s.write(&dump::header(&one_line(&plans)))?;
    }

    for plan in &plans {
        if cancelled() {
            stopped = true;
            break;
        }
        let mut own = match job.layout {
            Layout::FilePerDatabase => {
                let path = file_for(&job.target, &plan.db);
                let mut s = Sink::create(&path)?;
                s.write(&dump::header(&plan.db))?;
                Some(s)
            }
            Layout::SingleFile => None,
        };
        let out = own
            .as_mut()
            .or(sink.as_mut())
            .expect("one sink or the other");

        let (t, r, hit_stop) = write_database(
            conn,
            plan,
            &job.options,
            out,
            &mut report,
            &cancelled,
            &mut done,
            total,
        )
        .await?;
        tables += t;
        rows_total += r;
        stopped |= hit_stop;

        if let Some(s) = own {
            let mut s = s;
            s.write(&dump::footer())?;
            let path = s.path.display().to_string();
            bytes_written += s.finish()?;
            files.push(path);
        }
        if stopped {
            break;
        }
    }

    if let Some(mut s) = sink {
        s.write(&dump::footer())?;
        let path = s.path.display().to_string();
        bytes_written += s.finish()?;
        files.push(path);
    }

    // Read-only throughout, so there is nothing to commit — but the
    // transaction is ended explicitly rather than left for the drop, so the
    // server is not holding a read view while the connection is returned.
    let _ = conn.execute("COMMIT").await;

    if stopped {
        warnings.push(format!(
            "Cancelled after {rows_total} rows. The files hold what had been written by then."
        ));
    }
    report(Progress::Finished);
    Ok(DumpOutcome {
        files,
        bytes_written,
        tables,
        rows: rows_total,
        cancelled: stopped,
        warnings,
    })
}

/// One database's resolved selection.
struct Plan {
    db: String,
    tables: Vec<String>,
    views: Vec<String>,
    routines: Vec<schema::RoutineRef>,
    triggers: Vec<schema::TriggerRef>,
    events: Vec<schema::EventRef>,
}

impl Plan {
    fn objects(&self) -> usize {
        self.tables.len()
            + self.views.len()
            + self.routines.len()
            + self.triggers.len()
            + self.events.len()
    }
}

fn one_line(plans: &[Plan]) -> String {
    let names: Vec<&str> = plans.iter().map(|p| p.db.as_str()).collect();
    names.join(", ")
}

/// Where one database's script goes under `dir`, in the per-database layout.
///
/// **The only definition of that name.** [`run`] writes through here and
/// [`existing`] checks through here, because a confirmation that lists `a.sql`
/// before a run that writes `A.sql` is worse than no confirmation at all.
pub fn file_for(dir: &Path, db: &str) -> PathBuf {
    dir.join(format!("{}.sql", safe_file_name(db)))
}

/// Which of the files this export would write are already there.
///
/// Only the per-database layout needs asking: the save dialog does its own
/// overwrite confirmation for a single file, and a folder picker does not
/// (§5). Names only — the caller shows them, and a full path per database
/// would be the same directory repeated.
///
/// Unreadable directory, missing directory, a name that is really a
/// directory: all of them answer "not an existing file we would clobber", and
/// the write itself reports what it cannot do. This is a courtesy, not a
/// gate.
pub fn existing(dir: &Path, picks: &[DbPick]) -> Vec<String> {
    picks
        .iter()
        .filter(|p| file_for(dir, &p.db).is_file())
        .map(|p| {
            file_for(dir, &p.db)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.db.clone())
        })
        .collect()
}

/// A database name as a file name, without inventing a path.
///
/// MySQL allows characters in an identifier that no filesystem allows in a
/// name — a slash among them — so a database called `a/b` must not be able to
/// write `a/b.sql` two directories up.
fn safe_file_name(db: &str) -> String {
    db.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '_',
        })
        .collect()
}

/// Turn "all of this kind" into the actual names, now, inside the snapshot.
async fn resolve(
    conn: &mut MySqlConnection,
    pick: &DbPick,
    options: &DumpOptions,
    warnings: &mut Vec<String>,
) -> Result<Plan, String> {
    let (all_tables, all_views) = tables_and_views(conn, &pick.db).await?;

    let tables = match &pick.tables {
        Some(chosen) => keep(&all_tables, chosen),
        None => all_tables,
    };
    let views_unsorted = match &pick.views {
        Some(chosen) => keep(&all_views, chosen),
        None => all_views,
    };

    // Views, in an order that can actually be run.
    let views = if views_unsorted.len() < 2 {
        views_unsorted
    } else {
        match schema::view_dependencies_on(conn, &pick.db).await {
            Ok(edges) => match dump::order_views(&views_unsorted, &edges) {
                Ok(sorted) => sorted,
                Err(stuck) => {
                    warnings.push(format!(
                        "The views {} depend on each other in a way that could not be \
                         ordered, so they are written in name order and may need \
                         re-running.",
                        stuck.join(", ")
                    ));
                    views_unsorted
                }
            },
            Err(e) => {
                // information_schema.VIEW_TABLE_USAGE arrived in MySQL 8.0.13.
                // An older server is a reason to say so, not to refuse.
                warnings.push(format!(
                    "This server could not say what the views in {} depend on ({e}), so \
                     they are written in name order. A view that reads another view may \
                     need its statement re-running.",
                    pick.db
                ));
                views_unsorted
            }
        }
    };

    let routines = if options.contents.wants_schema() {
        let all = schema::routine_names_on(conn, &pick.db)
            .await
            .unwrap_or_default();
        match &pick.routines {
            Some(chosen) => {
                let want: BTreeSet<&str> = chosen.iter().map(|s| s.as_str()).collect();
                all.into_iter()
                    .filter(|r| want.contains(r.name.as_str()))
                    .collect()
            }
            None => all,
        }
    } else {
        Vec::new()
    };

    let triggers = if options.contents.wants_schema() {
        let all = dump::order_triggers(&schema::triggers_on(conn, &pick.db).await?[..]);
        match &pick.triggers {
            Some(chosen) => {
                let want: BTreeSet<&str> = chosen.iter().map(|s| s.as_str()).collect();
                all.into_iter()
                    .filter(|t| want.contains(t.name.as_str()))
                    .collect()
            }
            None => all,
        }
    } else {
        Vec::new()
    };

    let events = if options.contents.wants_schema() {
        let all = dump::order_events(&schema::events_on(conn, &pick.db).await?);
        match &pick.events {
            Some(chosen) => {
                let want: BTreeSet<&str> = chosen.iter().map(|s| s.as_str()).collect();
                all.into_iter()
                    .filter(|e| want.contains(e.name.as_str()))
                    .collect()
            }
            None => all,
        }
    } else {
        Vec::new()
    };

    Ok(Plan {
        db: pick.db.clone(),
        tables,
        views,
        routines,
        triggers,
        events,
    })
}

/// The chosen names, in the server's order rather than the caller's — so the
/// file is the same whatever order the UI happened to tick things in.
fn keep(all: &[String], chosen: &[String]) -> Vec<String> {
    let want: BTreeSet<&str> = chosen.iter().map(|s| s.as_str()).collect();
    all.iter()
        .filter(|n| want.contains(n.as_str()))
        .cloned()
        .collect()
}

async fn tables_and_views(
    conn: &mut MySqlConnection,
    db: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let rows = sqlx::query(
        "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
         WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
    )
    .bind(db)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| format!("Cannot list the objects in {db}: {e}"))?;

    let mut tables = Vec::new();
    let mut views = Vec::new();
    for r in rows {
        let name: String = r.try_get("TABLE_NAME").map_err(|e| e.to_string())?;
        let kind: String = r.try_get("TABLE_TYPE").map_err(|e| e.to_string())?;
        // Anything that is not a base table and not a view — a SEQUENCE, a
        // SYSTEM VIEW — is left out rather than guessed at.
        match kind.as_str() {
            "BASE TABLE" => tables.push(name),
            "VIEW" => views.push(name),
            _ => {}
        }
    }
    Ok((tables, views))
}

#[allow(clippy::too_many_arguments)]
async fn write_database(
    conn: &mut MySqlConnection,
    plan: &Plan,
    options: &DumpOptions,
    sink: &mut Sink,
    report: &mut impl FnMut(Progress),
    cancelled: &impl Fn() -> bool,
    done: &mut usize,
    total: usize,
) -> Result<(usize, u64, bool), String> {
    sink.write(&dump::database_section(&plan.db, options.create_database)?)?;

    let mut rows_total = 0u64;

    // 1. Tables, structure.
    if options.contents.wants_schema() {
        for table in &plan.tables {
            if cancelled() {
                return Ok((plan.tables.len(), rows_total, true));
            }
            *done += 1;
            report(Progress::Object {
                kind: "table".into(),
                db: plan.db.clone(),
                name: table.clone(),
                done: *done,
                total,
            });
            let ddl = schema::table_ddl_on(conn, &plan.db, table).await?;
            let drop = options
                .drop_existing
                .then(|| {
                    Ok::<String, String>(format!(
                        "DROP TABLE IF EXISTS {};",
                        crate::sqlgen::quote_ident(table)?
                    ))
                })
                .transpose()?;
            sink.write(&dump::object_section(
                &format!("Table {table}"),
                drop.as_deref(),
                &ddl,
                None,
                None,
            ))?;
        }
    }

    // 2. Data.
    if options.contents.wants_data() {
        for table in &plan.tables {
            if cancelled() {
                return Ok((plan.tables.len(), rows_total, true));
            }
            sink.write(&format!("\n-- Data for {table}\n"))?;
            let rows =
                stream_table(conn, &plan.db, table, options, sink, report, cancelled).await?;
            rows_total += rows;
        }
    }

    // 3. Views, after the tables they read.
    if options.contents.wants_schema() {
        for view in &plan.views {
            if cancelled() {
                return Ok((plan.tables.len(), rows_total, true));
            }
            *done += 1;
            report(Progress::Object {
                kind: "view".into(),
                db: plan.db.clone(),
                name: view.clone(),
                done: *done,
                total,
            });
            let ddl = schema::table_ddl_on(conn, &plan.db, view).await?;
            let ddl = if options.omit_definer {
                dump::strip_definer(&ddl)
            } else {
                ddl
            };
            let drop = options
                .drop_existing
                .then(|| {
                    Ok::<String, String>(format!(
                        "DROP VIEW IF EXISTS {};",
                        crate::sqlgen::quote_ident(view)?
                    ))
                })
                .transpose()?;
            sink.write(&dump::object_section(
                &format!("View {view}"),
                drop.as_deref(),
                &ddl,
                None,
                None,
            ))?;
        }

        // 4. Routines.
        for routine in &plan.routines {
            if cancelled() {
                return Ok((plan.tables.len(), rows_total, true));
            }
            *done += 1;
            report(Progress::Object {
                kind: "routine".into(),
                db: plan.db.clone(),
                name: routine.name.clone(),
                done: *done,
                total,
            });
            let script =
                schema::routine_ddl_on(conn, &plan.db, &routine.name, routine.kind).await?;
            let script = if options.omit_definer {
                dump::strip_definer(&script)
            } else {
                script
            };
            // `routine_script` already carries its own DROP and DELIMITER
            // block, so this is written as it is rather than wrapped again.
            sink.write(&format!("\n-- Routine {}\n{script}\n", routine.name))?;
        }

        // 5. Triggers — after the data, deliberately. Installed before it,
        //    each one fires once per inserted row and writes rows nobody
        //    asked for.
        for trigger in &plan.triggers {
            if cancelled() {
                return Ok((plan.tables.len(), rows_total, true));
            }
            *done += 1;
            report(Progress::Object {
                kind: "trigger".into(),
                db: plan.db.clone(),
                name: trigger.name.clone(),
                done: *done,
                total,
            });
            let def =
                schema::object_ddl_on(conn, &plan.db, &trigger.name, ObjectDdl::Trigger).await?;
            let body = if options.omit_definer {
                dump::strip_definer(&def.body)
            } else {
                def.body
            };
            let drop = options
                .drop_existing
                .then(|| {
                    Ok::<String, String>(format!(
                        "DROP TRIGGER IF EXISTS {};",
                        crate::sqlgen::quote_ident(&trigger.name)?
                    ))
                })
                .transpose()?;
            sink.write(&dump::object_section(
                &format!("Trigger {} on {}", trigger.name, trigger.table),
                drop.as_deref(),
                &body,
                Some(&def.sql_mode),
                None,
            ))?;
        }

        // 6. Events.
        for event in &plan.events {
            if cancelled() {
                return Ok((plan.tables.len(), rows_total, true));
            }
            *done += 1;
            report(Progress::Object {
                kind: "event".into(),
                db: plan.db.clone(),
                name: event.name.clone(),
                done: *done,
                total,
            });
            let def = schema::object_ddl_on(conn, &plan.db, &event.name, ObjectDdl::Event).await?;
            let body = if options.omit_definer {
                dump::strip_definer(&def.body)
            } else {
                def.body
            };
            let drop = options
                .drop_existing
                .then(|| {
                    Ok::<String, String>(format!(
                        "DROP EVENT IF EXISTS {};",
                        crate::sqlgen::quote_ident(&event.name)?
                    ))
                })
                .transpose()?;
            sink.write(&dump::object_section(
                &format!("Event {}", event.name),
                drop.as_deref(),
                &body,
                Some(&def.sql_mode),
                def.time_zone.as_deref(),
            ))?;
        }
    }

    Ok((plan.tables.len(), rows_total, false))
}

/// One table's rows, batched.
///
/// **Batched while streaming**, which `export::insert_row` does not do. A
/// batch of a thousand holds a few hundred kilobytes, not a table — and it is
/// the difference between a script that loads in 1.3s and one that takes 274s
/// (§9.5). A byte ceiling as well as a row count, because a thousand rows of
/// `TEXT` is not the same size as a thousand rows of `INT`.
async fn stream_table(
    conn: &mut MySqlConnection,
    db: &str,
    table: &str,
    options: &DumpOptions,
    sink: &mut Sink,
    report: &mut impl FnMut(Progress),
    cancelled: &impl Fn() -> bool,
) -> Result<u64, String> {
    let qualified = crate::sqlgen::qualify(db, table)?;
    let quoted = crate::sqlgen::quote_ident(table)?;

    let mut rows = 0u64;
    let mut in_batch = 0usize;
    let mut buf = String::new();

    let mut stream =
        sqlx::query(sqlx::AssertSqlSafe(format!("SELECT * FROM {qualified}"))).fetch(&mut *conn);

    while let Some(row) = stream.next().await {
        if cancelled() {
            break;
        }
        let row = row.map_err(|e| format!("Reading {qualified}: {e}"))?;
        let columns = columns_of(&row);
        let values: Vec<String> = (0..row.len())
            .map(|i| crate::export::row_literal(&row, i, &columns[i]))
            .collect::<Result<_, String>>()?;

        if in_batch == 0 {
            let names: Vec<String> = columns
                .iter()
                .map(|c| crate::sqlgen::quote_ident(&c.name))
                .collect::<Result<_, String>>()?;
            buf.push_str(&format!(
                "INSERT INTO {quoted} ({}) VALUES\n",
                names.join(", ")
            ));
        } else {
            buf.push_str(",\n");
        }
        buf.push_str(&format!("({})", values.join(", ")));
        in_batch += 1;
        rows += 1;

        if in_batch >= options.batch_rows || buf.len() >= options.batch_bytes {
            buf.push_str(";\n");
            sink.write(&buf)?;
            buf.clear();
            in_batch = 0;
            report(Progress::Rows {
                rows,
                bytes: sink.bytes,
            });
        }
    }
    if in_batch > 0 {
        buf.push_str(";\n");
        sink.write(&buf)?;
    }
    report(Progress::Rows {
        rows,
        bytes: sink.bytes,
    });
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database name is an identifier, not a path. MySQL allows a slash in
    /// one; a filesystem does not, and `a/b.sql` would be written somewhere
    /// nobody asked for.
    #[test]
    fn a_database_name_cannot_steer_the_file_somewhere_else() {
        assert_eq!(safe_file_name("poc"), "poc");
        assert_eq!(safe_file_name("../etc/passwd"), ".._etc_passwd");
        assert_eq!(safe_file_name("a/b"), "a_b");
        assert_eq!(safe_file_name("with space"), "with_space");
    }

    /// The selection decides *which*, the server decides *what order* — so a
    /// file does not change because somebody ticked things in a different
    /// sequence.
    #[test]
    fn chosen_names_come_out_in_the_servers_order() {
        let all = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let chosen = vec!["c".to_string(), "a".to_string()];
        assert_eq!(keep(&all, &chosen), vec!["a".to_string(), "c".to_string()]);
    }

    /// A name that is not there any more is not an error: the tree can be a
    /// few seconds stale, and refusing the whole export over one dropped
    /// table would be worse than exporting the rest.
    #[test]
    fn a_chosen_name_that_no_longer_exists_is_skipped() {
        let all = vec!["a".to_string()];
        let chosen = vec!["a".to_string(), "gone".to_string()];
        assert_eq!(keep(&all, &chosen), vec!["a".to_string()]);
    }

    fn tmp() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("db-query-dump-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pick(db: &str) -> DbPick {
        DbPick {
            db: db.to_string(),
            tables: None,
            views: None,
            routines: None,
            triggers: None,
            events: None,
        }
    }

    /// **The confirmation and the write agree on the name**, which is the only
    /// reason `file_for` exists: listing `a.sql` and then writing `A.sql`
    /// would be worse than not asking.
    #[test]
    fn the_file_checked_is_the_file_written() {
        let dir = tmp();
        std::fs::write(file_for(&dir, "with space"), "-- old\n").unwrap();
        assert_eq!(
            existing(&dir, &[pick("with space")]),
            vec!["with_space.sql"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only the ones that are there, in the order they were picked — and the
    /// others stay unmentioned rather than being listed as safe.
    #[test]
    fn only_the_files_already_there_are_named() {
        let dir = tmp();
        std::fs::write(dir.join("poc.sql"), "-- old\n").unwrap();
        std::fs::write(dir.join("stock.sql"), "-- old\n").unwrap();
        let picks = [pick("warehouse"), pick("poc"), pick("stock")];
        assert_eq!(existing(&dir, &picks), vec!["poc.sql", "stock.sql"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **A courtesy, not a gate.** An empty folder, a folder that is not
    /// there, a name taken by a directory: none of them is a file we would
    /// clobber, and none of them is a question worth asking. The write reports
    /// what it cannot do.
    #[test]
    fn nothing_to_overwrite_is_not_a_question() {
        let dir = tmp();
        assert!(existing(&dir, &[pick("poc")]).is_empty());
        std::fs::create_dir(dir.join("poc.sql")).unwrap();
        assert!(existing(&dir, &[pick("poc")]).is_empty());
        assert!(existing(&dir.join("not-there"), &[pick("poc")]).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
