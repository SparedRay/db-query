//! The scan against the real fixture folder, `dev/scripts`.
//!
//! Needs no server — which is exactly why it is here rather than postponed.
//! Stage 22 §9b.7 is the lesson: the export dialog's catalogue query had never
//! run against anything real, because the UI suite stubs `invoke` and the live
//! suite went through a different path, and a user found it in the first
//! minute. The dialog's own backend calls are the gap in the middle.
//!
//! The folder holds each case on purpose and `dev/scripts/README.md` says which
//! is which. Its files are read from disk here, so a fixture that drifts from
//! what the tests expect fails loudly rather than quietly agreeing.
//!
//!     cargo test --test sqlfiles_scan

use db_query_lib::exec::StatementKind;
use db_query_lib::sqlfiles::{self, Verdict};
use std::path::PathBuf;

fn folder() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `src-tauri`, so the repository root is its parent.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .join("dev/scripts")
}

fn scan_fixture(recursive: bool) -> sqlfiles::Scan {
    sqlfiles::read_and_scan(&folder(), recursive).expect("dev/scripts should be readable")
}

fn file<'a>(scan: &'a sqlfiles::Scan, path: &str) -> &'a sqlfiles::ScanFile {
    scan.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| {
            let all: Vec<&str> = scan.files.iter().map(|f| f.path.as_str()).collect();
            panic!("no {path} in the scan; found {all:?}")
        })
}

/// **The four numbers the preview leads with**, from real files.
///
/// Written out rather than computed, so a change to the fixture has to be a
/// deliberate change to this expectation too.
#[test]
fn the_fixture_folder_scans_to_the_numbers_the_preview_shows() {
    let s = scan_fixture(true);

    let paths: Vec<&str> = s.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "01-setup.sql",
            "02-orders.sql",
            "03-report.sql",
            "04-proc.sql",
            "05-notes.sql",
            "06-no-semicolon.sql",
            "07-bom.sql",
            "99-teardown.sql",
            "daily/08-nested.sql",
        ],
        "alphabetical by path, which is why people number their scripts"
    );
    // README.md is not SQL and is simply not here — a folder with a readme in
    // it is not a folder with a problem.
    assert!(!paths.iter().any(|p| p.ends_with(".md")));

    // 04 cannot be split and 05 has nothing to run, so seven of the nine run.
    assert_eq!(s.runnable_files, 7);
    // 01: 3 · 02: 1 · 03: 3 · 06: 1 · 07: 1 · 99: 2 · daily/08: 1
    assert_eq!(s.statements, 12);
    // 02, 03×3, 06, 07, daily/08.
    assert_eq!(s.outputs, 7);
    // 01's DROP, CREATE, INSERT and 99's two DROPs.
    assert_eq!(s.writes, 5);
}

/// Not recursive leaves the subfolder alone, which is the checkbox's whole job.
#[test]
fn without_subfolders_the_nested_script_is_not_there() {
    let s = scan_fixture(false);
    assert!(
        !s.files.iter().any(|f| f.path.contains('/')),
        "a nested file was picked up with recursion off"
    );
    assert_eq!(s.files.len(), 8);
}

/// **One CSV per result set, mirrored into subfolders, no `-1` on the first.**
#[test]
fn the_output_names_are_what_the_report_will_show() {
    let s = scan_fixture(true);
    let outputs: Vec<&str> = s
        .files
        .iter()
        .flat_map(|f| &f.statements)
        .filter_map(|st| st.output.as_deref())
        .collect();
    assert_eq!(
        outputs,
        [
            "02-orders.csv",
            "03-report.csv",
            "03-report-2.csv",
            "03-report-3.csv",
            "06-no-semicolon.csv",
            "07-bom.csv",
            "daily/08-nested.csv",
        ]
    );
    // And every one is unique, which is what stops a silent overwrite.
    let mut sorted = outputs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), outputs.len());
}

/// **The BOM'd file is runnable and its statement is the `SELECT`.**
///
/// §9.7 measured MySQL refusing a leading U+FEFF, so this is the case that
/// would otherwise fail on somebody's folder with error 1064 pointing at a
/// character they cannot see. The fixture file really does start with the three
/// BOM bytes — checked here so a reformat cannot quietly remove the case.
#[test]
fn the_bom_file_is_a_real_bom_and_scans_clean() {
    let raw = std::fs::read(folder().join("07-bom.sql")).unwrap();
    assert_eq!(
        &raw[..3],
        &[0xEF, 0xBB, 0xBF],
        "07-bom.sql has lost its BOM, so it no longer tests anything"
    );

    let s = scan_fixture(false);
    let f = file(&s, "07-bom.sql");
    assert_eq!(f.verdict, Verdict::Ready);
    assert_eq!(f.statements.len(), 1);
    let st = &f.statements[0];
    assert_eq!(st.preview, "SELECT amount FROM batch_probe WHERE id = 3");
    assert!(st.returns_rows);
    assert_eq!(st.output.as_deref(), Some("07-bom.csv"));
}

/// A file whose last statement has no terminator still has that statement —
/// ordinary in a file, rare in an editor buffer (§9.1).
#[test]
fn a_file_with_no_trailing_semicolon_keeps_its_statement() {
    let s = scan_fixture(false);
    let f = file(&s, "06-no-semicolon.sql");
    assert_eq!(f.verdict, Verdict::Ready);
    assert_eq!(f.statements.len(), 1);
    assert_eq!(
        f.statements[0].preview,
        "SELECT label FROM batch_probe WHERE id = 1"
    );
}

/// The two files that will not run are **named with their reason**, not
/// dropped. A report silent about a file somebody put in the folder leaves them
/// wondering whether it ran.
#[test]
fn the_files_that_will_not_run_say_why() {
    let s = scan_fixture(false);
    assert_eq!(file(&s, "04-proc.sql").verdict, Verdict::CannotSplit);
    assert!(file(&s, "04-proc.sql").statements.is_empty());
    assert_eq!(file(&s, "05-notes.sql").verdict, Verdict::NothingToRun);
    assert!(file(&s, "05-notes.sql").statements.is_empty());
}

/// **The confirmation's list**, as it will read: every writing statement with
/// its file, line and first line of code — and nothing that does not write.
#[test]
fn the_writing_statements_name_themselves_with_a_line() {
    let s = scan_fixture(true);
    // The preview is compared by its opening rather than in full: where
    // exactly 72 characters falls is `PREVIEW_CHARS`' business and already has
    // its own test, and hand-counting it here only produced a wrong
    // expectation.
    let writes: Vec<(String, usize, String)> = s
        .files
        .iter()
        .flat_map(|f| f.statements.iter().map(move |st| (f.path.clone(), st)))
        .filter(|(_, st)| st.writes)
        .map(|(p, st)| (p, st.line, st.preview.chars().take(32).collect::<String>()))
        .collect();

    let expected = [
        ("01-setup.sql", 2, "DROP TABLE IF EXISTS batch_probe"),
        ("01-setup.sql", 3, "CREATE TABLE batch_probe (id INT"),
        ("01-setup.sql", 4, "INSERT INTO batch_probe VALUES ("),
        ("99-teardown.sql", 2, "DROP PROCEDURE IF EXISTS batch_p"),
        ("99-teardown.sql", 3, "DROP TABLE IF EXISTS batch_probe"),
    ];
    assert_eq!(writes.len(), expected.len(), "{writes:#?}");
    for (got, want) in writes.iter().zip(expected) {
        assert_eq!((got.0.as_str(), got.1, got.2.as_str()), want);
    }

    // **Line 2, not line 1.** Each of these files opens with a comment, and the
    // span owns it (§9.1) — so a confirmation built on `span.start` would send
    // somebody to the comment instead of the statement.
    assert!(writes.iter().all(|(_, line, _)| *line >= 2));
}

/// Every statement's kind, in order, from real text — the classification the
/// write count and the CSV count are both derived from.
#[test]
fn the_kinds_are_what_the_counts_are_built_on() {
    let s = scan_fixture(true);
    let kinds: Vec<(&str, Vec<StatementKind>)> = s
        .files
        .iter()
        .filter(|f| !f.statements.is_empty())
        .map(|f| {
            (
                f.path.as_str(),
                f.statements.iter().map(|st| st.kind).collect(),
            )
        })
        .collect();

    use StatementKind::*;
    assert_eq!(
        kinds,
        [
            ("01-setup.sql", vec![Other, Other, Modify]),
            ("02-orders.sql", vec![Select]),
            // The third is `SHOW TABLES LIKE`, which returns rows and must
            // never be rewritten — a different kind for that reason.
            ("03-report.sql", vec![Select, Select, RowReturning]),
            ("06-no-semicolon.sql", vec![Select]),
            ("07-bom.sql", vec![Select]),
            ("99-teardown.sql", vec![Other, Other]),
            ("daily/08-nested.sql", vec![Select]),
        ]
    );
}

/// **The runner will send these exact bytes.** The offsets are `split`'s, so
/// slicing the file with them gives the statement back — comment and all, which
/// MySQL accepts (§9.7) and which keeps the preview and the run identical.
#[test]
fn the_offsets_slice_back_out_of_the_real_files() {
    let s = scan_fixture(true);
    for f in &s.files {
        if f.statements.is_empty() {
            continue;
        }
        let text = std::fs::read_to_string(folder().join(&f.path)).unwrap();
        // The file on disk still has its BOM; what was scanned did not.
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
        for st in &f.statements {
            assert!(
                st.end <= text.len(),
                "{}: offset {} is past the end",
                f.path,
                st.end
            );
            let sliced = &text[st.start..st.end];
            assert!(
                sliced.contains(st.preview.trim_end_matches('…').trim()),
                "{}: {:?} is not in {:?}",
                f.path,
                st.preview,
                sliced
            );
        }
    }
}
