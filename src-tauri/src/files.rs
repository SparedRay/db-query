//! Opening and saving script files.
//!
//! Two ways to lose someone's work, both quiet, both worse than a crash:
//!
//!   * **Saving a lossily-decoded buffer.** A file that is not valid UTF-8 gets
//!     replacement characters when we decode it. Writing that back destroys the
//!     original bytes. Such files are opened with `encoding: "utf-8-lossy"` and
//!     the UI refuses to Save over them — Save As to a new file is fine.
//!   * **Clobbering a file that changed on disk.** `save_file` takes the mtime
//!     we saw at open and reports `Conflict` rather than overwriting.
//!
//! Writes are atomic: content goes to a sibling temp file and is renamed over
//! the target, so a crash mid-write cannot leave a truncated script.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

/// Refuse outright above this. A dump this size loaded into CodeMirror freezes
/// the UI, and freezing is exactly what this project exists to avoid. Refusing
/// with a readable message is the honest answer.
pub const MAX_OPEN_BYTES: u64 = 25 * 1024 * 1024;

/// Above this we still open, but the UI warns and (Phase 7) backs off linting.
pub const WARN_OPEN_BYTES: u64 = 2 * 1024 * 1024;

/// Dialect used when a file's extension is not in the registry.
pub const DEFAULT_DIALECT: &str = "mysql";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileTypeSpec {
    pub label: String,
    pub extensions: Vec<String>,
    pub dialect: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedFile {
    pub path: String,
    pub name: String,
    /// Always normalised to `\n` for the editor. `line_ending` records what to
    /// write back.
    pub contents: String,
    pub size_bytes: u64,
    pub dialect: String,
    /// `"utf-8"` | `"utf-8-lossy"`. Lossy means Save must be disabled.
    pub encoding: String,
    /// `"lf"` | `"crlf"` — preserved on save.
    pub line_ending: String,
    pub mtime_ms: u64,
    /// Over `WARN_OPEN_BYTES`; the UI should warn and ease off linting.
    pub large: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    pub path: String,
    pub name: String,
    pub mtime_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SaveOutcome {
    Saved {
        mtime_ms: u64,
    },
    /// The file changed underneath us. The caller must ask the user before
    /// anything is overwritten.
    Conflict {
        disk_mtime_ms: u64,
    },
}

/// The file types we can open, and the dialect each implies.
///
/// This is the extensibility hook: when another engine is added it appends its
/// own entry here, and the open dialog's filters, the extension → dialect
/// mapping, and the editor's syntax mode all follow from it. Nothing else
/// hard-codes a file extension.
pub fn supported_file_types() -> Vec<FileTypeSpec> {
    vec![FileTypeSpec {
        label: "SQL".into(),
        extensions: vec!["sql".into(), "ddl".into(), "dml".into(), "mysql".into()],
        dialect: "mysql".into(),
    }]
}

pub fn dialect_for_path(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    supported_file_types()
        .into_iter()
        .find(|spec| spec.extensions.contains(&ext))
        .map(|spec| spec.dialect)
        .unwrap_or_else(|| DEFAULT_DIALECT.to_string())
}

fn mtime_ms(path: &Path) -> Result<u64, String> {
    let meta = fs::metadata(path).map_err(|e| format!("Cannot stat file: {e}"))?;
    let modified = meta
        .modified()
        .map_err(|e| format!("Cannot read modification time: {e}"))?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("untitled")
        .to_string()
}

fn human_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if n as f64 >= MB {
        format!("{:.1} MB", n as f64 / MB)
    } else {
        format!("{:.0} KB", n as f64 / 1024.0)
    }
}

/// Detect the dominant line ending, then normalise the text to `\n`.
///
/// Normalising matters because CodeMirror counts a `\r\n` as two characters,
/// which would put every byte offset from the splitter out of step with the
/// editor's positions.
fn normalise_newlines(text: &str) -> (String, &'static str) {
    let crlf = text.matches("\r\n").count();
    let total_lf = text.matches('\n').count();
    let lone_lf = total_lf.saturating_sub(crlf);
    let ending = if crlf > lone_lf { "crlf" } else { "lf" };
    (text.replace("\r\n", "\n"), ending)
}

fn restore_newlines(text: &str, line_ending: &str) -> String {
    if line_ending.eq_ignore_ascii_case("crlf") {
        // Normalise first so we never emit `\r\r\n` if the buffer already has one.
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

pub fn read_file(path: &Path) -> Result<OpenedFile, String> {
    let meta = fs::metadata(path).map_err(|e| format!("Cannot open file: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{} is a directory, not a file.", file_name(path)));
    }

    let size = meta.len();
    if size > MAX_OPEN_BYTES {
        return Err(format!(
            "{} is {} — too large to open (limit {}). Large dumps are better run through the mysql CLI.",
            file_name(path),
            human_bytes(size),
            human_bytes(MAX_OPEN_BYTES),
        ));
    }

    let bytes = fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;
    let (raw, encoding) = match String::from_utf8(bytes) {
        Ok(s) => (s, "utf-8"),
        // Keep going rather than refusing — but the caller must not save over it.
        Err(e) => (
            String::from_utf8_lossy(e.as_bytes()).into_owned(),
            "utf-8-lossy",
        ),
    };
    let (contents, line_ending) = normalise_newlines(&raw);

    Ok(OpenedFile {
        path: path.to_string_lossy().into_owned(),
        name: file_name(path),
        contents,
        size_bytes: size,
        dialect: dialect_for_path(path),
        encoding: encoding.to_string(),
        line_ending: line_ending.to_string(),
        mtime_ms: mtime_ms(path)?,
        large: size > WARN_OPEN_BYTES,
    })
}

/// Write `contents` to `path`, refusing if the file changed since `expect_mtime`.
///
/// `expect_mtime: None` means "no expectation" — used by Save As, where the
/// user has already been shown an overwrite prompt by the native dialog.
pub fn save_file(
    path: &Path,
    contents: &str,
    expect_mtime: Option<u64>,
    line_ending: &str,
) -> Result<SaveOutcome, String> {
    if let Some(expected) = expect_mtime {
        if path.exists() {
            let disk = mtime_ms(path)?;
            if disk != expected {
                return Ok(SaveOutcome::Conflict {
                    disk_mtime_ms: disk,
                });
            }
        }
    }

    let body = restore_newlines(contents, line_ending);
    write_atomic(path, body.as_bytes())?;
    Ok(SaveOutcome::Saved {
        mtime_ms: mtime_ms(path)?,
    })
}

/// Write via a sibling temp file and rename, so a crash mid-write cannot leave
/// a half-written script where the original was. The temp file must live in the
/// same directory or the rename could cross a filesystem boundary and stop
/// being atomic.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp: PathBuf = dir.join(format!(
        ".{}.dbq-tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("save")
    ));

    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Cannot write file: {e}"));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Cannot replace file: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "db-query-files-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn extension_maps_to_dialect_via_the_registry() {
        assert_eq!(dialect_for_path(Path::new("a.sql")), "mysql");
        assert_eq!(dialect_for_path(Path::new("a.SQL")), "mysql");
        assert_eq!(dialect_for_path(Path::new("a.ddl")), "mysql");
        // Unknown extensions still open, on the default dialect.
        assert_eq!(dialect_for_path(Path::new("notes.txt")), DEFAULT_DIALECT);
        assert_eq!(dialect_for_path(Path::new("noext")), DEFAULT_DIALECT);
    }

    #[test]
    fn registry_is_the_only_place_extensions_live() {
        let types = supported_file_types();
        assert!(!types.is_empty());
        for spec in &types {
            assert!(
                !spec.extensions.is_empty(),
                "{} has no extensions",
                spec.label
            );
            assert!(
                spec.extensions.iter().all(|e| !e.starts_with('.')),
                "extensions are bare, no leading dot"
            );
        }
    }

    #[test]
    fn reads_a_plain_utf8_file() {
        let d = tmpdir();
        let p = d.join("q.sql");
        fs::write(&p, "SELECT 1;\nSELECT 2;\n").unwrap();
        let f = read_file(&p).unwrap();
        assert_eq!(f.name, "q.sql");
        assert_eq!(f.encoding, "utf-8");
        assert_eq!(f.line_ending, "lf");
        assert_eq!(f.dialect, "mysql");
        assert!(!f.large);
        assert_eq!(f.contents, "SELECT 1;\nSELECT 2;\n");
    }

    #[test]
    fn crlf_files_round_trip_without_rewriting_every_line() {
        let d = tmpdir();
        let p = d.join("win.sql");
        let original = "SELECT 1;\r\nSELECT 2;\r\n";
        fs::write(&p, original).unwrap();

        let f = read_file(&p).unwrap();
        assert_eq!(f.line_ending, "crlf");
        // The editor sees LF, so byte offsets stay in step with CodeMirror.
        assert_eq!(f.contents, "SELECT 1;\nSELECT 2;\n");

        save_file(&p, &f.contents, Some(f.mtime_ms), &f.line_ending).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), original);
    }

    #[test]
    fn saving_an_lf_file_does_not_introduce_crlf() {
        let d = tmpdir();
        let p = d.join("unix.sql");
        fs::write(&p, "SELECT 1;\n").unwrap();
        let f = read_file(&p).unwrap();
        save_file(&p, "SELECT 2;\n", Some(f.mtime_ms), &f.line_ending).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "SELECT 2;\n");
    }

    #[test]
    fn crlf_restore_is_idempotent() {
        // Guards against emitting \r\r\n if the buffer already contains CRLF.
        assert_eq!(restore_newlines("a\r\nb", "crlf"), "a\r\nb");
        assert_eq!(restore_newlines("a\nb", "crlf"), "a\r\nb");
        assert_eq!(restore_newlines("a\r\nb", "lf"), "a\r\nb");
    }

    #[test]
    fn mixed_endings_pick_the_dominant_one() {
        let (_, e) = normalise_newlines("a\r\nb\r\nc\n");
        assert_eq!(e, "crlf");
        let (_, e) = normalise_newlines("a\nb\nc\r\n");
        assert_eq!(e, "lf");
        let (_, e) = normalise_newlines("no newlines at all");
        assert_eq!(e, "lf");
    }

    #[test]
    fn invalid_utf8_opens_lossily_and_is_flagged() {
        let d = tmpdir();
        let p = d.join("latin1.sql");
        // 0xFF is not valid UTF-8.
        fs::write(&p, b"SELECT '\xFF';\n").unwrap();
        let f = read_file(&p).unwrap();
        assert_eq!(
            f.encoding, "utf-8-lossy",
            "must be flagged so Save is disabled"
        );
        assert!(f.contents.contains('\u{FFFD}'));
    }

    #[test]
    fn refuses_a_file_over_the_size_limit() {
        let d = tmpdir();
        let p = d.join("huge.sql");
        let f = fs::File::create(&p).unwrap();
        f.set_len(MAX_OPEN_BYTES + 1).unwrap();
        drop(f);
        let err = read_file(&p).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert!(
            err.contains("mysql CLI"),
            "should say what to do instead: {err}"
        );
    }

    #[test]
    fn flags_a_large_but_openable_file() {
        let d = tmpdir();
        let p = d.join("big.sql");
        let f = fs::File::create(&p).unwrap();
        f.set_len(WARN_OPEN_BYTES + 1).unwrap();
        drop(f);
        assert!(read_file(&p).unwrap().large);
    }

    #[test]
    fn reading_a_directory_is_a_readable_error_not_a_panic() {
        let d = tmpdir();
        let err = read_file(&d).unwrap_err();
        assert!(err.contains("is a directory"), "{err}");
    }

    #[test]
    fn refuses_to_clobber_a_file_that_changed_on_disk() {
        let d = tmpdir();
        let p = d.join("shared.sql");
        fs::write(&p, "SELECT 1;\n").unwrap();
        let f = read_file(&p).unwrap();

        // Someone else edits it. Bump the mtime explicitly so the test does not
        // depend on filesystem timestamp granularity.
        fs::write(&p, "SELECT 999;\n").unwrap();
        let bumped = f.mtime_ms + 5_000;
        filetime_set(&p, bumped);

        match save_file(&p, "SELECT 2;\n", Some(f.mtime_ms), "lf").unwrap() {
            SaveOutcome::Conflict { disk_mtime_ms } => assert_eq!(disk_mtime_ms, bumped),
            SaveOutcome::Saved { .. } => panic!("clobbered a file that changed on disk"),
        }
        // And the other edit survives untouched.
        assert_eq!(fs::read_to_string(&p).unwrap(), "SELECT 999;\n");
    }

    #[test]
    fn saves_when_the_file_is_unchanged() {
        let d = tmpdir();
        let p = d.join("mine.sql");
        fs::write(&p, "SELECT 1;\n").unwrap();
        let f = read_file(&p).unwrap();
        match save_file(&p, "SELECT 2;\n", Some(f.mtime_ms), "lf").unwrap() {
            SaveOutcome::Saved { .. } => {}
            SaveOutcome::Conflict { .. } => panic!("false conflict on an unchanged file"),
        }
        assert_eq!(fs::read_to_string(&p).unwrap(), "SELECT 2;\n");
    }

    #[test]
    fn save_as_to_a_new_path_needs_no_mtime() {
        let d = tmpdir();
        let p = d.join("brand-new.sql");
        save_file(&p, "SELECT 1;\n", None, "lf").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "SELECT 1;\n");
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_behind() {
        let d = tmpdir();
        let p = d.join("clean.sql");
        save_file(&p, "SELECT 1;\n", None, "lf").unwrap();
        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("dbq-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    /// Set mtime without pulling in the `filetime` crate — one more dependency
    /// is not worth it for a single test helper.
    fn filetime_set(path: &Path, ms: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }
}
