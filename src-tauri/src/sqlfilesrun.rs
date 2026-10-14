//! Running a folder of SQL scripts: one connection, one plan, CSVs out.
//!
//! [`crate::sqlfiles`] decides *what* would happen; this makes it happen. The
//! split matters because the dialog's confirmation is built from the plan
//! (§4.1), so the plan has to exist before anything runs and the runner must
//! not re-derive it.
//!
//! Shaped like [`crate::dumprun`] on purpose — its own connection, `Progress`
//! over a channel, cancel polled between units of work — because it is the same
//! kind of thing and a second shape would be a second thing to learn.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sqlx::MySqlConnection;

use crate::engine::Capabilities;
use crate::exec::{self, StatementKind};
use crate::export::{self, CsvOptions};
use crate::sqlfiles::{Scan, ScanFile, ScanStatement, Verdict};

/// Everything the runner needs that is not a connection.
pub struct Job<'a> {
    /// The plan the dialog showed. **Not recomputed here**: a confirmation
    /// that described a different run would be worse than none.
    pub scan: &'a Scan,
    pub source: PathBuf,
    pub dest: PathBuf,
    /// The database to start in, as the dialog named it (§3.1).
    pub database: Option<String>,
    pub csv: CsvOptions,
    /// Checked before each statement is sent, so a read-only connection
    /// refuses a write plainly instead of relaying the server's complaint.
    pub caps: &'a Capabilities,
}

/// One CSV that was written.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvWritten {
    /// Relative to the destination, `/`-separated.
    pub name: String,
    pub rows: usize,
    pub bytes: u64,
    /// Stop was pressed while this one was streaming, so it holds the rows it
    /// had and not the rest. **Said out loud**: a CSV that is silently partial
    /// is one somebody opens in a spreadsheet and believes.
    pub partial: bool,
}

/// What became of one file.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FileStatus {
    Ran,
    /// It was never going to run, and why — a `DELIMITER` block, nothing to
    /// run, or bytes we would have had to guess at.
    Skipped {
        reason: String,
    },
    /// It failed, and where. The run stops here (§4.3).
    Failed {
        /// 1-based, within the file.
        statement: usize,
        /// The line its **code** starts on, not the comment above it.
        line: usize,
        message: String,
    },
    /// The run had already stopped before this file was reached.
    ///
    /// The row a report-by-omission leaves out, and the reason stop-on-error is
    /// honest rather than quiet: five successes and one failure, with nothing
    /// said about the two files never attempted, is a report that misleads.
    NotRun,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReport {
    pub path: String,
    /// The database this file actually ran in, which a `USE` in an earlier file
    /// can have changed (§3.4).
    pub database: Option<String>,
    pub status: FileStatus,
    pub csvs: Vec<CsvWritten>,
    pub statements_run: usize,
    /// Summed over the statements that write.
    pub rows_affected: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    pub files: Vec<FileReport>,
    pub dest: String,
    pub csvs: usize,
    pub rows: usize,
    pub bytes: u64,
    /// Stop was pressed.
    pub cancelled: bool,
    /// `05-bad.sql:12`, when a statement failed.
    pub stopped_at: Option<String>,
}

/// What the strip is told while it runs.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Progress {
    /// Known before anything runs, because the plan already existed.
    Planned {
        files: usize,
        statements: usize,
        outputs: usize,
    },
    File {
        path: String,
        done: usize,
        total: usize,
    },
    Statement {
        file: String,
        index: usize,
        of: usize,
        preview: String,
    },
    /// Rows and bytes for the statement in flight.
    Rows { rows: usize, bytes: u64 },
}

/// Why a file will not run, as a sentence for the report.
///
/// `None` means it will. Pure, so the four wordings are a test rather than
/// something to read off the screen later.
pub fn skip_reason(verdict: &Verdict) -> Option<String> {
    match verdict {
        Verdict::Ready => None,
        Verdict::NothingToRun => Some("Nothing to run — comments only.".into()),
        Verdict::CannotSplit => Some(
            "Contains a DELIMITER block, which cannot be split into statements. \
             Run it from an editor tab instead."
                .into(),
        ),
        Verdict::CannotRead { reason } => Some(reason.clone()),
    }
}

/// Run the plan.
///
/// `report` is called as it goes; `cancelled` is polled between files, between
/// statements and between rows, so Stop lands inside a big result rather than
/// after it (§9.3 proved the connection survives that).
pub async fn run(
    conn: &mut MySqlConnection,
    job: &Job<'_>,
    mut report: impl FnMut(Progress),
    cancelled: impl Fn() -> bool,
) -> Result<RunOutcome, String> {
    // **No wrapping transaction, and none promised.** MySQL commits implicitly
    // on DDL, so a folder containing one `CREATE TABLE` cannot be atomic — and
    // a promise that breaks on the first `CREATE` is worse than no promise
    // (§4.3). What is guaranteed is only that nothing after a failure runs.
    let mut current_db = match &job.database {
        Some(db) => {
            use_database(conn, db).await?;
            Some(db.clone())
        }
        // Whatever the connection opened on. Read once, over the text protocol
        // — see `database_now` for why not the ordinary one.
        None => database_now(conn).await?,
    };

    report(Progress::Planned {
        files: job.scan.runnable_files,
        statements: job.scan.statements,
        outputs: job.scan.outputs,
    });

    let total = job.scan.files.len();
    let mut files: Vec<FileReport> = Vec::with_capacity(total);
    let mut csvs = 0usize;
    let mut rows_total = 0usize;
    let mut bytes_total = 0u64;
    let mut stopped = false;
    let mut stopped_at = None;
    let mut made: BTreeSet<PathBuf> = BTreeSet::new();

    for (i, file) in job.scan.files.iter().enumerate() {
        if stopped || cancelled() {
            stopped = true;
            files.push(not_run(file, &current_db));
            continue;
        }

        if let Some(reason) = skip_reason(&file.verdict) {
            files.push(FileReport {
                path: file.path.clone(),
                database: current_db.clone(),
                status: FileStatus::Skipped { reason },
                csvs: Vec::new(),
                statements_run: 0,
                rows_affected: 0,
            });
            continue;
        }

        report(Progress::File {
            path: file.path.clone(),
            done: i + 1,
            total,
        });

        let text = std::fs::read_to_string(crate::sqlfiles::under(&job.source, &file.path))
            .map_err(|e| format!("Cannot re-read {}: {e}", file.path))?;
        // The plan was made from the text with its BOM already off, so the
        // offsets are relative to that. Taking it off again here keeps the two
        // in step — and if the file changed on disk since the scan, the slice
        // below is what notices.
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

        let mut written: Vec<CsvWritten> = Vec::new();
        let mut affected = 0u64;
        let mut ran = 0usize;
        let mut failure = None;

        for (n, st) in file.statements.iter().enumerate() {
            if cancelled() {
                stopped = true;
                break;
            }
            let Some(sql) = text.get(st.start..st.end) else {
                failure = Some(FileStatus::Failed {
                    statement: n + 1,
                    line: st.line,
                    message: "The file changed on disk since it was scanned. \
                              Re-open the folder and try again."
                        .into(),
                });
                break;
            };

            // Before it is sent, not after it is refused (§5).
            if let Some(why) = crate::engine::refusal(job.caps, st.kind) {
                failure = Some(FileStatus::Failed {
                    statement: n + 1,
                    line: st.line,
                    message: why,
                });
                break;
            }

            report(Progress::Statement {
                file: file.path.clone(),
                index: n + 1,
                of: file.statements.len(),
                preview: st.preview.clone(),
            });

            match run_statement(conn, job, st, sql, &mut made, &cancelled, &mut report).await {
                Ok(Ran::Rows(csv)) => {
                    rows_total += csv.rows;
                    bytes_total += csv.bytes;
                    csvs += 1;
                    if csv.partial {
                        stopped = true;
                    }
                    written.push(csv);
                    ran += 1;
                }
                Ok(Ran::Affected(n_rows)) => {
                    affected += n_rows;
                    ran += 1;
                }
                Err(message) => {
                    failure = Some(FileStatus::Failed {
                        statement: n + 1,
                        line: st.line,
                        message,
                    });
                    break;
                }
            }

            // A `USE` inside a script moves every later statement, and every
            // later *file*, somewhere else (§9.5). Re-read rather than parsed:
            // the server is the authority on where we are, and parsing `USE`
            // out of arbitrary SQL is a second grammar to maintain.
            if st.kind == StatementKind::Session && starts_with_use(sql) {
                current_db = database_now(conn).await?;
            }
        }

        let status = match failure {
            Some(f) => {
                stopped_at = Some(match &f {
                    FileStatus::Failed { line, .. } => format!("{}:{}", file.path, line),
                    _ => file.path.clone(),
                });
                stopped = true;
                f
            }
            None => FileStatus::Ran,
        };
        files.push(FileReport {
            path: file.path.clone(),
            database: current_db.clone(),
            status,
            csvs: written,
            statements_run: ran,
            rows_affected: affected,
        });
    }

    Ok(RunOutcome {
        files,
        dest: job.dest.to_string_lossy().into_owned(),
        csvs,
        rows: rows_total,
        bytes: bytes_total,
        cancelled: cancelled(),
        stopped_at,
    })
}

/// What one statement did.
enum Ran {
    Rows(CsvWritten),
    Affected(u64),
}

async fn run_statement(
    conn: &mut MySqlConnection,
    job: &Job<'_>,
    st: &ScanStatement,
    sql: &str,
    made: &mut BTreeSet<PathBuf>,
    cancelled: &impl Fn() -> bool,
    report: &mut impl FnMut(Progress),
) -> Result<Ran, String> {
    let Some(name) = st.output.as_deref() else {
        // Not a row-returning statement: run it and keep the count.
        //
        // **`raw_sql`, not `query`.** A script holds anything — `CREATE
        // PROCEDURE`, `USE`, `SET`, `FLUSH` — and sqlx's ordinary path prepares
        // the statement, which MySQL refuses for some of those. The text
        // protocol runs whatever the server would run from a client.
        let done = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(&mut *conn)
            .await
            .map_err(|e| crate::session::friendly(&e))?;
        return Ok(Ran::Affected(done.rows_affected()));
    };

    let path = crate::sqlfiles::under(&job.dest, name);
    if let Some(parent) = path.parent() {
        // Mirrored subfolders (§4.4) need the directory. Created once per
        // directory rather than per CSV.
        if made.insert(parent.to_path_buf()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
        }
    }

    let opts = job.csv.clone();
    let header_opts = opts.clone();
    let (rows, bytes, partial) = exec::stream_rows_to_file(
        conn,
        sql,
        &path,
        move |row, buf| {
            buf.push_str(&export::csv_row(row, &opts));
            Ok(())
        },
        move |row| Ok(export::csv_header(row, &header_opts)),
        cancelled,
        |rows, bytes| report(Progress::Rows { rows, bytes }),
    )
    .await?;

    Ok(Ran::Rows(CsvWritten {
        name: name.to_string(),
        rows,
        bytes,
        partial,
    }))
}

/// `USE` the named database, over the text protocol.
async fn use_database(conn: &mut MySqlConnection, db: &str) -> Result<(), String> {
    let quoted = crate::sqlgen::quote_ident(db)?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("USE {quoted}")))
        .execute(conn)
        .await
        .map_err(|e| crate::session::friendly(&e))?;
    Ok(())
}

/// Which database this connection is in, **asked over the text protocol**.
///
/// Not `sqlx::query`, which prepares: MySQL resolves `DATABASE()` when a
/// statement is prepared and sqlx caches prepared statements per connection by
/// SQL text, so the ordinary path replays the answer from before the first
/// `USE` — with no error at all. Stage 23 §9.5 measured exactly that, and a
/// report confidently naming the wrong database is the bug it would cause.
async fn database_now(conn: &mut MySqlConnection) -> Result<Option<String>, String> {
    use sqlx::Row as _;
    sqlx::raw_sql(sqlx::AssertSqlSafe("SELECT DATABASE()".to_string()))
        .fetch_one(conn)
        .await
        .map_err(|e| crate::session::friendly(&e))?
        .try_get::<Option<String>, _>(0)
        .map_err(|e| format!("Cannot read the current database: {e}"))
}

/// Is this statement a `USE`?
///
/// Through `exec::leading_keyword_of`, which masks comments and takes the whole
/// first **word** — because the obvious version, comparing the first three
/// bytes, says yes to `USER_COUNT()`. Its own test caught that, which is the
/// reason it is written this way rather than the short way.
fn starts_with_use(sql: &str) -> bool {
    exec::leading_keyword_of(sql) == "USE"
}

fn not_run(file: &ScanFile, db: &Option<String>) -> FileReport {
    FileReport {
        path: file.path.clone(),
        database: db.clone(),
        status: FileStatus::NotRun,
        csvs: Vec::new(),
        statements_run: 0,
        rows_affected: 0,
    }
}

/// Where a path sits relative to `dir`, for the report.
pub fn relative_to(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every reason a file will not run has words.** The report shows these,
    /// so "skipped" on its own is never what somebody reads.
    #[test]
    fn each_reason_a_file_is_skipped_is_a_sentence() {
        assert_eq!(skip_reason(&Verdict::Ready), None);
        assert!(skip_reason(&Verdict::NothingToRun)
            .unwrap()
            .contains("comments only"));
        let delim = skip_reason(&Verdict::CannotSplit).unwrap();
        assert!(delim.contains("DELIMITER"), "{delim}");
        // And it says what to do instead, rather than only what it will not do.
        assert!(delim.contains("editor tab"), "{delim}");
        assert_eq!(
            skip_reason(&Verdict::CannotRead {
                reason: "Not valid UTF-8".into()
            })
            .unwrap(),
            "Not valid UTF-8",
            "the reader's own reason, not a paraphrase of it"
        );
    }

    /// `USE` is recognised through the comment a span owns (§9.1), because
    /// that is the shape it arrives in.
    #[test]
    fn a_use_is_recognised_behind_its_comment() {
        assert!(starts_with_use("USE other"));
        assert!(starts_with_use("use `other`"));
        assert!(starts_with_use("-- switch over\nUSE other"));
        assert!(starts_with_use("/* switch */ USE other"));
        assert!(!starts_with_use("SELECT 1"));
        assert!(!starts_with_use("-- USE is mentioned here\nSELECT 1"));
        // Not a false positive on a word that merely starts with those bytes.
        assert!(!starts_with_use("USER_COUNT()"));
        assert!(!starts_with_use(""));
    }

    #[test]
    fn a_report_path_is_relative_and_slash_separated() {
        assert_eq!(
            relative_to(Path::new("/tmp/out"), Path::new("/tmp/out/daily/a.csv")),
            "daily/a.csv"
        );
        // Not under it: shown whole rather than mangled.
        assert_eq!(
            relative_to(Path::new("/tmp/out"), Path::new("/elsewhere/a.csv")),
            "/elsewhere/a.csv"
        );
    }
}
