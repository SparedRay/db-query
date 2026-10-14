//! Reading a folder of SQL scripts, and deciding what running it would do.
//!
//! This is the **plan**, and it exists as its own module for one reason: the
//! confirmation in the dialog and the work the runner does must come from the
//! same answer. "9 statements will write" is only checkable if the nine it
//! names are the nine that run (Stage 23 §4.1), and the way to guarantee that
//! is to compute them once.
//!
//! Everything below [`scan`] is **pure** — file contents in, a plan out — so
//! every claim the preview makes is a unit test. The directory walk and the
//! reading are [`collect`] and the caller's, and they are deliberately the
//! only impure part.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::exec::{self, StatementKind};
use crate::split;

/// How much of a statement the write confirmation shows.
///
/// Long enough to recognise the statement, short enough that a dozen of them
/// fit in a dialog. A `DELETE` with a 400-character `WHERE` is identified by
/// its first line; the file and line number are there for the rest.
const PREVIEW_CHARS: usize = 72;

/// One file as it arrives: a path relative to the chosen folder, and either its
/// text or why it could not be read.
///
/// A `Result` rather than an `Option` because *why* matters — "not valid UTF-8"
/// and "permission denied" are different things to tell somebody, and a folder
/// where one file is unreadable is still a folder worth running.
pub struct ScanInput {
    pub path: String,
    pub text: Result<String, String>,
}

/// What one statement is, for both the preview and the run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatement {
    /// Byte offsets into the file's text, exactly as `split` gave them — so
    /// the runner sends the same bytes the preview counted.
    pub start: usize,
    pub end: usize,
    /// 1-based line of the first byte of **code**, not of the span.
    ///
    /// A span owns the comment above it (§9.1), so `span.start` would name the
    /// comment's line and a person looking at line 12 would find
    /// `-- rebuild the totals` where the error said `DELETE`.
    pub line: usize,
    pub kind: StatementKind,
    /// It returns rows, so it produces a CSV.
    pub returns_rows: bool,
    /// It writes, so the confirmation names it (§4.1).
    ///
    /// The same test `engine::refusal` applies on a read-only connection, so
    /// the number the dialog shows and the statements the runner refuses cannot
    /// drift apart.
    pub writes: bool,
    /// The statement's first line of code, for the confirmation.
    pub preview: String,
    /// Where its CSV goes, relative to the destination folder. `None` for a
    /// statement that returns no rows.
    pub output: Option<String>,
}

/// Why a file will or will not be run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Verdict {
    /// It has statements, and they are in `ScanFile::statements`.
    Ready,
    /// Zero statements: comments and whitespace (§9.2). Shown rather than
    /// dropped — a file somebody put in the folder and that will do nothing is
    /// worth a line.
    NothingToRun,
    /// A `DELIMITER` directive, which makes `split` return the whole buffer as
    /// one span by design. Per-statement naming is then impossible, and such a
    /// file is almost always a routine definition that returns no rows (§7).
    CannotSplit,
    /// The bytes are not text we are willing to guess at.
    CannotRead { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanFile {
    /// Relative to the source folder, always with `/` separators so a report
    /// reads the same on both platforms.
    pub path: String,
    pub verdict: Verdict,
    pub statements: Vec<ScanStatement>,
}

/// The whole plan, and the four numbers the preview leads with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    pub files: Vec<ScanFile>,
    /// Files that will actually run — not the same as `files.len()`.
    pub runnable_files: usize,
    pub statements: usize,
    /// CSVs this run would write.
    pub outputs: usize,
    /// Statements that write. **The number the confirmation is about.**
    pub writes: usize,
}

/// Plan a run over files that have already been read.
///
/// Order is the caller's: §4.2 promises files run in the order they are listed,
/// and this preserves it rather than sorting again, so the preview and the run
/// agree with each other *and* with what the dialog showed.
pub fn scan(inputs: &[ScanInput]) -> Scan {
    let mut files = Vec::with_capacity(inputs.len());
    // **Output names are unique across the whole run, not per file.** Mirroring
    // subfolders (§4.4) makes a collision almost impossible, but "almost" here
    // means a silent overwrite of somebody's results, so the names are taken
    // from a set rather than assumed free.
    let mut taken: BTreeSet<String> = BTreeSet::new();

    for input in inputs {
        let text = match &input.text {
            Ok(t) => t,
            Err(reason) => {
                files.push(ScanFile {
                    path: input.path.clone(),
                    verdict: Verdict::CannotRead {
                        reason: reason.clone(),
                    },
                    statements: Vec::new(),
                });
                continue;
            }
        };

        let out = split::split(text);
        if out.delimiter_detected {
            files.push(ScanFile {
                path: input.path.clone(),
                verdict: Verdict::CannotSplit,
                statements: Vec::new(),
            });
            continue;
        }

        let mut statements = Vec::new();
        for span in &out.statements {
            let span_text = &text[span.start..span.end];
            // A span with no code at all is not a statement. `split` does not
            // produce one (§9.2) but a file is not obliged to be reasonable,
            // and sending an empty string to the server would be the worst
            // available answer.
            let Some(code_at) = split::first_code_byte(span_text) else {
                continue;
            };
            let code = &span_text[code_at..];
            let kind = exec::classify(code);
            let returns_rows = exec::returns_rows(kind);
            let output = if returns_rows {
                Some(claim_output(&input.path, &mut taken))
            } else {
                None
            };
            statements.push(ScanStatement {
                start: span.start,
                end: span.end,
                line: split::line_of(text, span.start + code_at),
                kind,
                returns_rows,
                writes: writes(kind),
                preview: preview_of(code),
                output,
            });
        }

        let verdict = if statements.is_empty() {
            Verdict::NothingToRun
        } else {
            Verdict::Ready
        };
        files.push(ScanFile {
            path: input.path.clone(),
            verdict,
            statements,
        });
    }

    let runnable_files = files.iter().filter(|f| f.verdict == Verdict::Ready).count();
    let statements: usize = files.iter().map(|f| f.statements.len()).sum();
    let outputs = files
        .iter()
        .flat_map(|f| &f.statements)
        .filter(|s| s.output.is_some())
        .count();
    let writes = files
        .iter()
        .flat_map(|f| &f.statements)
        .filter(|s| s.writes)
        .count();

    Scan {
        files,
        runnable_files,
        statements,
        outputs,
        writes,
    }
}

/// Does this statement write?
///
/// **Deliberately the same test `engine::refusal` uses** for a read-only
/// connection: `Modify` or `Other`. If these two ever disagree, the dialog
/// would count writes the runner then refuses, or — far worse — stay silent
/// about one it is about to perform.
///
/// `Session` is not a write. `SET`, `USE` and `START TRANSACTION` change this
/// connection and nothing in anybody's data, and listing them in a
/// confirmation about writes would teach people to skim it.
fn writes(kind: StatementKind) -> bool {
    matches!(kind, StatementKind::Modify | StatementKind::Other)
}

/// The CSV name for the next result of `sql_path`, avoiding anything taken.
///
/// `report.sql` gives `report.csv`, then `report-2.csv`, `report-3.csv`.
/// **No `-1` on the first**: a folder of one-`SELECT` files is the common case
/// and should produce clean names (§4.4).
fn claim_output(sql_path: &str, taken: &mut BTreeSet<String>) -> String {
    let stem = sql_path
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(sql_path);
    let mut n = 1usize;
    loop {
        let candidate = if n == 1 {
            format!("{stem}.csv")
        } else {
            format!("{stem}-{n}.csv")
        };
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// The first line of a statement's code, for the confirmation.
///
/// One line because that is what identifies a statement to somebody skimming;
/// the file and line number carry the rest. Internal whitespace is collapsed so
/// a statement formatted across a line break still reads as one phrase.
fn preview_of(code: &str) -> String {
    let first_line = code.lines().next().unwrap_or("").trim();
    let collapsed: String = {
        let mut out = String::with_capacity(first_line.len());
        let mut space = false;
        for c in first_line.chars() {
            if c.is_whitespace() {
                space = true;
            } else {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
        out
    };
    // By characters, not bytes: truncating UTF-8 by byte count panics, and a
    // statement naming an accented table is not an edge case.
    if collapsed.chars().count() <= PREVIEW_CHARS {
        return collapsed;
    }
    let kept: String = collapsed.chars().take(PREVIEW_CHARS).collect();
    format!("{kept}…")
}

/// The `.sql` files under `dir`, sorted, as paths relative to it.
///
/// Sorted because §4.2 promises files run in the order they are listed, and
/// alphabetical is the order people get when they name things `01-setup.sql`.
/// Relative, with `/` separators, so the report and the mirrored output paths
/// read the same on both platforms.
///
/// Anything that is not a readable `.sql` file is simply not here: this picks
/// what to run, and a folder holding a `README.md` is not a folder with a
/// problem.
pub fn collect(dir: &Path, recursive: bool) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    walk(dir, dir, recursive, &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(root: &Path, dir: &Path, recursive: bool, out: &mut Vec<String>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        // `file_type` rather than `metadata`: it does not follow symlinks, so a
        // link pointing back at an ancestor cannot send this round for ever.
        let kind = entry
            .file_type()
            .map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?;
        if kind.is_dir() {
            if recursive {
                walk(root, &path, recursive, out)?;
            }
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let is_sql = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("sql"));
        if is_sql {
            if let Some(rel) = relative(root, &path) {
                out.push(rel);
            }
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// Read every `.sql` file under `dir` and plan the run.
///
/// Reads through [`crate::files::read_file`], which is what makes the BOM come
/// off (§9.7) and the size limit apply — a script too big to hold is a script
/// too big to split, and that limit's own message already says what to do
/// instead.
///
/// **A file we would have to guess at is not a file we run.** `read_file`
/// answers `utf-8-lossy` rather than failing, which is right for the editor
/// (you can look at it and decide) and wrong here: nobody is looking, and the
/// replacement characters would be sent to the server as SQL.
pub fn read_and_scan(dir: &Path, recursive: bool) -> Result<Scan, String> {
    let names = collect(dir, recursive)?;
    let inputs: Vec<ScanInput> = names
        .into_iter()
        .map(|rel| {
            let text = crate::files::read_file(&under(dir, &rel)).and_then(|f| {
                if f.encoding == "utf-8-lossy" {
                    Err("Not valid UTF-8 — the file would have to be guessed at.".to_string())
                } else {
                    Ok(f.contents)
                }
            });
            ScanInput { path: rel, text }
        })
        .collect();
    Ok(scan(&inputs))
}

/// Join a `/`-separated relative name onto `dir`.
///
/// Used for both ends: a scanned `.sql` under the source folder and a planned
/// `.csv` under the destination. One function because mirroring (§4.4) only
/// means anything if the two agree about what a relative name is.
pub fn under(dir: &Path, relative_name: &str) -> PathBuf {
    let mut p = dir.to_path_buf();
    for part in relative_name.split('/') {
        p.push(part);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str, text: &str) -> ScanInput {
        ScanInput {
            path: path.to_string(),
            text: Ok(text.to_string()),
        }
    }

    fn outputs(scan: &Scan) -> Vec<&str> {
        scan.files
            .iter()
            .flat_map(|f| &f.statements)
            .filter_map(|s| s.output.as_deref())
            .collect()
    }

    /// **One CSV per result set, and no `-1` on the first.** A folder of
    /// one-`SELECT` files is the common case and gets clean names; a file with
    /// three results gets three files rather than losing two of them.
    #[test]
    fn each_result_set_gets_its_own_csv() {
        let s = scan(&[
            input("orders.sql", "SELECT 1;"),
            input("report.sql", "SELECT 1;\nSELECT 2;\nSELECT 3;"),
        ]);
        assert_eq!(
            outputs(&s),
            ["orders.csv", "report.csv", "report-2.csv", "report-3.csv"]
        );
        assert_eq!(s.outputs, 4);
        assert_eq!(s.statements, 4);
    }

    /// Subfolders are mirrored, so two files with the same name in different
    /// directories do not overwrite one another — which is the normal shape of
    /// a repository of scripts, not an edge case.
    #[test]
    fn subfolders_are_mirrored_rather_than_flattened() {
        let s = scan(&[
            input("daily/report.sql", "SELECT 1;"),
            input("weekly/report.sql", "SELECT 1;"),
        ]);
        assert_eq!(outputs(&s), ["daily/report.csv", "weekly/report.csv"]);
    }

    /// And if two names *would* still collide, the second is moved rather than
    /// quietly overwriting the first.
    #[test]
    fn a_colliding_output_name_is_moved_not_overwritten() {
        let s = scan(&[input("a.sql", "SELECT 1;"), input("a.sql", "SELECT 2;")]);
        assert_eq!(outputs(&s), ["a.csv", "a-2.csv"]);
    }

    /// **The number the confirmation is about.** Writes are counted, and they
    /// are counted by the same test `engine::refusal` applies — so the dialog
    /// cannot promise a run the connection will refuse, nor stay quiet about a
    /// write it is about to do.
    #[test]
    fn writes_are_counted_and_nothing_else_is() {
        let s = scan(&[input(
            "mixed.sql",
            "SELECT 1;\n\
             INSERT INTO t VALUES (1);\n\
             UPDATE t SET n = 1;\n\
             DELETE FROM t;\n\
             CREATE TABLE u (id INT);\n\
             DROP TABLE u;\n\
             SET @x = 1;\n\
             START TRANSACTION;\n\
             SHOW TABLES;",
        )]);
        assert_eq!(s.statements, 9);
        // INSERT, UPDATE, DELETE, CREATE, DROP.
        assert_eq!(s.writes, 5);
        // SELECT and SHOW.
        assert_eq!(s.outputs, 2);

        let kinds: Vec<(StatementKind, bool, bool)> = s.files[0]
            .statements
            .iter()
            .map(|st| (st.kind, st.writes, st.returns_rows))
            .collect();
        // `SET` and `START TRANSACTION` are neither: they change this
        // connection and nothing in anybody's data.
        assert_eq!(kinds[6], (StatementKind::Session, false, false));
        assert_eq!(kinds[7], (StatementKind::Session, false, false));
        assert_eq!(kinds[8], (StatementKind::RowReturning, false, true));
    }

    /// **The property, not a hand-counted number.**
    ///
    /// `writes` says the comment above it is the same test `engine::refusal`
    /// applies on a read-only connection. A comment cannot keep that true, so
    /// this asserts it for every `StatementKind` there is: whatever the dialog
    /// calls a write is exactly what such a connection refuses.
    ///
    /// If they drift, one of two bad things has happened — the dialog promises
    /// a run that will be refused, or, far worse, it says nothing about a write
    /// it is about to perform.
    #[test]
    fn a_write_is_exactly_what_a_read_only_connection_refuses() {
        let mut caps = crate::engine::Capabilities::mysql();
        caps.writes = false;
        caps.read_only = true;

        for kind in [
            StatementKind::Select,
            StatementKind::RowReturning,
            StatementKind::Modify,
            StatementKind::Session,
            StatementKind::Other,
        ] {
            assert_eq!(
                writes(kind),
                crate::engine::refusal(&caps, kind).is_some(),
                "{kind:?} is counted and refused differently"
            );
        }
    }

    /// **The line is the code's, not the comment's** — §9.1's finding, and the
    /// reason `first_code_byte` exists. A person told "line 12" must find the
    /// statement there, not the note above it.
    #[test]
    fn a_statement_is_reported_at_the_line_its_code_starts_on() {
        let s = scan(&[input(
            "one.sql",
            "-- a header\n\
             -- two lines of it\n\
             SELECT 1;\n\
             \n\
             -- and a note about the delete\n\
             DELETE FROM t;\n",
        )]);
        let st = &s.files[0].statements;
        assert_eq!(st[0].line, 3);
        assert_eq!(st[0].preview, "SELECT 1");
        assert_eq!(st[1].line, 6);
        assert_eq!(st[1].preview, "DELETE FROM t");
    }

    /// A file of comments is **listed**, with a verdict, rather than dropped.
    /// Somebody put it in the folder; a report that does not mention it leaves
    /// them wondering whether it ran.
    #[test]
    fn a_file_with_nothing_to_run_is_still_reported() {
        let s = scan(&[
            input("notes.sql", "-- just a note\n"),
            input("real.sql", "SELECT 1;"),
        ]);
        assert_eq!(s.files[0].verdict, Verdict::NothingToRun);
        assert!(s.files[0].statements.is_empty());
        assert_eq!(s.runnable_files, 1, "only `real.sql` will run");
        assert_eq!(s.files.len(), 2, "both are listed");
    }

    /// A `DELIMITER` block cannot be split per statement, so it is named with
    /// that reason instead of being half-run (§7).
    #[test]
    fn a_delimiter_block_is_named_and_not_run() {
        let s = scan(&[input(
            "proc.sql",
            "DELIMITER //\n\
             CREATE PROCEDURE p() BEGIN SELECT 1; END //\n\
             DELIMITER ;\n",
        )]);
        assert_eq!(s.files[0].verdict, Verdict::CannotSplit);
        assert!(s.files[0].statements.is_empty());
        assert_eq!(s.writes, 0, "nothing is promised about a file we will skip");
        assert_eq!(s.runnable_files, 0);
    }

    /// An unreadable file keeps its reason. "Not valid UTF-8" and "permission
    /// denied" are different things to tell somebody.
    #[test]
    fn an_unreadable_file_keeps_why() {
        let s = scan(&[ScanInput {
            path: "binary.sql".into(),
            text: Err("not valid UTF-8".into()),
        }]);
        assert_eq!(
            s.files[0].verdict,
            Verdict::CannotRead {
                reason: "not valid UTF-8".into()
            }
        );
        assert_eq!(s.runnable_files, 0);
    }

    /// **The runner sends the bytes the preview counted.** The offsets are
    /// `split`'s own, so slicing the file with them gives back the statement —
    /// including the comment the span owns, which MySQL is happy with (§9.7).
    #[test]
    fn the_offsets_slice_back_to_the_statement() {
        let text = "-- note\nSELECT 1;\nINSERT INTO t VALUES (2);";
        let s = scan(&[input("f.sql", text)]);
        let st = &s.files[0].statements;
        assert_eq!(&text[st[0].start..st[0].end], "-- note\nSELECT 1");
        assert_eq!(&text[st[1].start..st[1].end], "INSERT INTO t VALUES (2)");
    }

    /// The preview is one line, collapsed, and truncated by **characters**.
    /// Truncating UTF-8 by byte count panics, and a statement naming an
    /// accented table is not an edge case.
    #[test]
    fn a_long_statement_is_previewed_by_characters() {
        let long = format!("DELETE FROM `tablé` WHERE naïve IN ({})", "1, ".repeat(60));
        let s = scan(&[input("f.sql", &format!("{long};"))]);
        let p = &s.files[0].statements[0].preview;
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1, "{p}");
        assert!(p.ends_with('…'));
        assert!(p.starts_with("DELETE FROM `tablé`"), "{p}");
    }

    /// A statement written across several lines still reads as one phrase.
    #[test]
    fn a_wrapped_statement_previews_as_its_first_line() {
        let s = scan(&[input("f.sql", "UPDATE   t\n   SET n = 1\n   WHERE id = 2;")]);
        assert_eq!(s.files[0].statements[0].preview, "UPDATE t");
    }

    /// Order is the caller's, because the dialog showed it and §4.2 promised
    /// it. Nothing here re-sorts.
    #[test]
    fn the_order_given_is_the_order_kept() {
        let s = scan(&[
            input("02-second.sql", "SELECT 1;"),
            input("01-first.sql", "SELECT 1;"),
        ]);
        let paths: Vec<&str> = s.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["02-second.sql", "01-first.sql"]);
    }

    /// An empty folder is a plan that does nothing, not an error.
    #[test]
    fn nothing_at_all_is_an_empty_plan() {
        let s = scan(&[]);
        assert_eq!(s.statements, 0);
        assert_eq!(s.writes, 0);
        assert_eq!(s.outputs, 0);
        assert_eq!(s.runnable_files, 0);
        assert!(s.files.is_empty());
    }

    // ------------------------------------------------------- the directory walk

    fn tmp() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("db-query-sqlfiles-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_walk_finds_sql_files_sorted_and_relative() {
        let dir = tmp();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for (p, body) in [
            ("02-b.sql", "SELECT 2;"),
            ("01-a.sql", "SELECT 1;"),
            ("README.md", "not sql"),
            ("upper.SQL", "SELECT 3;"),
            ("sub/deep.sql", "SELECT 4;"),
        ] {
            std::fs::write(dir.join(p), body).unwrap();
        }

        // Not recursive: the subfolder is left alone, and the extension test
        // ignores case because a Windows-authored folder will have `.SQL` in it.
        assert_eq!(
            collect(&dir, false).unwrap(),
            ["01-a.sql", "02-b.sql", "upper.SQL"]
        );
        // Recursive: and the relative path keeps the folder, which is what
        // makes the mirrored output name (§4.4).
        assert_eq!(
            collect(&dir, true).unwrap(),
            ["01-a.sql", "02-b.sql", "sub/deep.sql", "upper.SQL"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A missing folder says so rather than reporting an empty one — "nothing
    /// to do" and "that is not there" are different answers.
    #[test]
    fn a_missing_folder_is_an_error_not_an_empty_list() {
        let dir = tmp().join("not-there");
        let err = collect(&dir, false).unwrap_err();
        assert!(err.contains("Cannot read"), "{err}");
    }

    /// The path is built from the relative name, so mirroring survives into
    /// the filesystem — at both ends.
    #[test]
    fn a_relative_name_joins_the_same_way_at_both_ends() {
        let dest = Path::new("/tmp/out");
        assert_eq!(
            under(dest, "daily/report.csv"),
            Path::new("/tmp/out/daily/report.csv")
        );
        assert_eq!(under(dest, "flat.csv"), Path::new("/tmp/out/flat.csv"));
    }

    /// **Reading is where the BOM comes off**, because it goes through
    /// `files::read_file` — so the first statement of a Notepad-saved script is
    /// the `SELECT`, not an invisible character followed by one (§9.7).
    #[test]
    fn reading_a_folder_strips_a_bom_and_refuses_what_it_cannot_decode() {
        let dir = tmp();
        std::fs::write(dir.join("a-bom.sql"), "\u{feff}SELECT 1;\n").unwrap();
        // Invalid UTF-8: a lone 0x80 continuation byte.
        std::fs::write(dir.join("b-binary.sql"), [b'S', 0x80, b';']).unwrap();
        std::fs::write(dir.join("c-plain.sql"), "SELECT 2;\n").unwrap();

        let s = read_and_scan(&dir, false).unwrap();
        assert_eq!(s.files.len(), 3);

        // The BOM is gone from the bytes the runner will send.
        assert_eq!(s.files[0].verdict, Verdict::Ready);
        let st = &s.files[0].statements[0];
        assert_eq!(st.preview, "SELECT 1");
        assert_eq!(st.line, 1);

        // And the undecodable one is named with a reason rather than guessed
        // at. `read_file` answers "lossy" rather than failing, which is right
        // for the editor — somebody can look at it — and wrong here, where
        // nobody is looking and the replacement characters would be sent as
        // SQL.
        match &s.files[1].verdict {
            Verdict::CannotRead { reason } => assert!(reason.contains("UTF-8"), "{reason}"),
            other => panic!("expected CannotRead, got {other:?}"),
        }

        assert_eq!(s.files[2].verdict, Verdict::Ready);
        assert_eq!(s.runnable_files, 2);
        assert_eq!(s.outputs, 2);
        assert_eq!(outputs(&s), ["a-bom.csv", "c-plain.csv"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
