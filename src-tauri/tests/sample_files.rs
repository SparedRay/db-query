//! Checks the repo's `dev/samples/` fixtures really have the properties the
//! manual click-through relies on. Cheap insurance against a fixture being
//! silently normalised by an editor or a git checkout.

use db_query_lib::files;
use std::path::PathBuf;

fn sample(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dev/samples")
        .join(name)
}

#[test]
fn plain_sql_sample_opens_as_utf8_lf() {
    let f = files::read_file(&sample("report.sql")).unwrap();
    assert_eq!(f.encoding, "utf-8");
    assert_eq!(f.line_ending, "lf");
    assert_eq!(f.dialect, "mysql");
    assert!(!f.large);
    assert!(f.contents.contains("LEFT JOIN orders"));
}

#[test]
fn crlf_sample_is_detected_and_normalised_for_the_editor() {
    let f = files::read_file(&sample("crlf.sql")).unwrap();
    assert_eq!(f.line_ending, "crlf", "fixture lost its CRLF endings");
    // The editor must see LF, or byte offsets drift from CodeMirror positions.
    assert!(!f.contents.contains('\r'), "CRLF leaked into the buffer");
}

#[test]
fn latin1_sample_is_flagged_so_save_is_disabled() {
    let f = files::read_file(&sample("latin1.sql")).unwrap();
    assert_eq!(
        f.encoding, "utf-8-lossy",
        "fixture is no longer invalid UTF-8, so it cannot exercise the Save guard"
    );
    assert!(f.contents.contains('\u{FFFD}'));
}
