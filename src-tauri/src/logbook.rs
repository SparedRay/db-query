//! What the app did, kept where somebody can copy it out.
//!
//! # Why this exists
//!
//! Before Stage 16 this app logged nothing at all — one `println!`, inside a
//! test. In a packaged build the webview console goes nowhere, so a frontend
//! error on a user's machine was simply lost, and a backend failure could only
//! ever be diagnosed by reproducing it here. Stage 15's Flyway report was a
//! keyhole cut because there was no window.
//!
//! # The rule that shapes the whole module
//!
//! Three commands take a password or a token as an argument, and a general log
//! is open-ended by design — the opposite of how everything else here treats
//! secrets. So:
//!
//!   * **`note` takes a sentence, never a structure.** There is no entry point
//!     that accepts arguments and formats them. A call site says what happened
//!     in words it chose, which is what keeps a password out *by construction*
//!     rather than by filtering. An API that took `&impl Debug` would write
//!     down whatever somebody adds to that struct next year.
//!   * **The scrubber fails closed.** It is the second line, not the first: it
//!     catches the mistake, drops the whole entry rather than editing it, and
//!     leaves a line saying so — `history.rs` reached the same two conclusions
//!     for the same reason, and an unexplained gap is worse than a known one.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

/// Entries held in memory. The diagnostics button copies these; the file keeps
/// more. Two thousand is about a long session's worth of user-driven commands
/// — nothing here polls, so the rate is the rate somebody clicks.
pub const CAPACITY: usize = 2000;

/// Rotate the file past this. One predecessor is kept, so the worst case on
/// disk is twice this and never more.
pub const ROTATE_AT: u64 = 1024 * 1024;

pub const FILE_NAME: &str = "db-query.log";
pub const PREVIOUS_FILE_NAME: &str = "db-query.log.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Fixed width, so the messages in a pasted log line up and can be read
    /// down the page rather than across it.
    fn label(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info ",
            Level::Warn => "warn ",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// UTC, to the second. A log read on another continent is read in UTC or
    /// it is read wrongly.
    pub at: String,
    pub level: Level,
    /// Where it came from: `ui`, `flyway`, `mysql`, `elastic`, `mcp`,
    /// `update`, `app`. A short free word rather than an enum — the log is
    /// prose, and a new area should not need a type to change.
    pub area: String,
    pub message: String,
}

impl Entry {
    /// One line of a pasted log.
    pub fn line(&self) -> String {
        format!(
            "{} {} {:<8} {}",
            self.at,
            self.level.label(),
            self.area,
            self.message
        )
    }
}

// ------------------------------------------------------------ the scrubber

/// What a withheld entry is replaced by. An absence that explains itself.
const WITHHELD: &str = "[entry withheld: it matched a credential pattern]";

/// Does this message look like it is carrying a secret?
///
/// **Deliberately shape-based, not word-based.** `Access denied for user
/// 'root'@'localhost' (using password: YES)` is the most useful line MySQL
/// ever prints and it contains the word "password"; what it does not contain
/// is a *value*. So each rule below looks for a credential next to its value,
/// and a rule that cannot tell the difference is not a rule worth having.
pub fn looks_like_a_secret(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();

    // `scheme://user:secret@host` — the classic way a password ends up in a
    // log, because a connection URL is the obvious thing to print.
    if let Some(start) = lower.find("://") {
        let rest = &message[start + 3..];
        if let Some(at) = rest.find('@') {
            let authority = &rest[..at];
            // A colon in the userinfo means a password is present. `host:port`
            // has no `@` after it, so it cannot reach here.
            if authority.contains(':') && !authority.contains('/') {
                return true;
            }
        }
    }

    // SQL that sets one.
    if lower.contains("identified by") || lower.contains("identified with") {
        return true;
    }

    // `Bearer <something>` — an MCP or API token.
    if let Some(i) = lower.find("bearer ") {
        if !message[i + 7..].trim().is_empty() {
            return true;
        }
    }

    // `password=x`, `password: x`, `apikey=x`, `api_key: x`, `token=x`.
    for key in [
        "password", "passwd", "api_key", "apikey", "api key", "token", "secret",
    ] {
        for sep in ['=', ':'] {
            let needle = format!("{key}{sep}");
            let mut from = 0;
            while let Some(i) = lower[from..].find(&needle) {
                let at = from + i + needle.len();
                let value = message[at..].trim_start();
                // Trailing punctuation is part of the sentence, not of the
                // value: MySQL's `(using password: YES)` is the line this
                // scrubber most has to leave alone, and `YES)` is not `YES`.
                let word = value
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches([')', ']', '.', ',', ';', '\'', '"']);
                // A value that is itself a report of presence, not the thing.
                let harmless = word.is_empty()
                    || word.eq_ignore_ascii_case("yes")
                    || word.eq_ignore_ascii_case("no")
                    || word.eq_ignore_ascii_case("none")
                    || word.eq_ignore_ascii_case("null")
                    || word.starts_with("***")
                    || word.starts_with("<");
                if !harmless {
                    return true;
                }
                from = at;
            }
        }
    }
    false
}

// ------------------------------------------------------------- the logbook

/// The ring, and where it spills to.
#[derive(Debug, Default)]
pub struct Logbook {
    entries: VecDeque<Entry>,
    /// Where entries are appended. `None` keeps the book in memory only, which
    /// is what every test does and what the app does before it knows its own
    /// config directory.
    file: Option<PathBuf>,
    /// Counted rather than logged, because logging a failure to log is a loop.
    pub write_failures: u64,
}

impl Logbook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start appending to `dir`.
    ///
    /// The directory is created if it is not there. `config_dir` does not make
    /// it — it is made by whichever module writes first — and on a brand new
    /// install that is nobody, until a profile is saved. A log that is silent
    /// on first run is silent for exactly the person most likely to need it.
    pub fn open_in(&mut self, dir: &Path) {
        let _ = fs::create_dir_all(dir);
        self.file = Some(dir.join(FILE_NAME));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record one entry, or the fact that one was withheld.
    pub fn note(&mut self, level: Level, area: &str, message: &str) {
        let entry = Entry {
            at: now(),
            level,
            area: area.to_string(),
            message: if looks_like_a_secret(message) {
                WITHHELD.to_string()
            } else {
                message.to_string()
            },
        };
        self.append(&entry);
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// The whole ring, oldest first.
    /// Where entries are being appended, if anywhere.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn lines(&self) -> Vec<String> {
        self.entries.iter().map(Entry::line).collect()
    }

    /// The last `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        self.entries
            .iter()
            .skip(self.entries.len().saturating_sub(n))
            .map(Entry::line)
            .collect()
    }

    /// Append to the file, rotating first if it has grown past `ROTATE_AT`.
    ///
    /// **A failure here is counted and swallowed.** The log is a diagnostic,
    /// and an app that cannot start because it cannot write its log has turned
    /// a diagnostic into a dependency.
    fn append(&mut self, entry: &Entry) {
        let Some(path) = self.file.clone() else {
            return;
        };
        if let Err(_e) = self.write(&path, entry) {
            self.write_failures += 1;
        }
    }

    fn write(&self, path: &Path, entry: &Entry) -> std::io::Result<()> {
        if let Ok(meta) = fs::metadata(path) {
            if meta.len() >= ROTATE_AT {
                let previous = path.with_file_name(PREVIOUS_FILE_NAME);
                // Rename rather than truncate: whatever was being read stays
                // readable, and one predecessor is the whole retention policy.
                let _ = fs::rename(path, previous);
            }
        }
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{}", entry.line())
    }
}

/// UTC, to the second, as `2026-09-16 14:03:11Z`.
///
/// Space rather than `T`, and no sub-second part: this is read by people in a
/// chat window, not parsed.
fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%SZ").to_string()
}

// ------------------------------------------------------------- the one book

static BOOK: OnceLock<Mutex<Logbook>> = OnceLock::new();

fn book() -> &'static Mutex<Logbook> {
    BOOK.get_or_init(|| Mutex::new(Logbook::new()))
}

/// Record something. The whole public surface, and it takes words.
///
/// A poisoned lock is ignored rather than propagated — see `append`: the log
/// must not be able to take the app down.
pub fn note(level: Level, area: &str, message: impl AsRef<str>) {
    if let Ok(mut b) = book().lock() {
        b.note(level, area, message.as_ref());
    }
}

pub fn info(area: &str, message: impl AsRef<str>) {
    note(Level::Info, area, message)
}

pub fn warn(area: &str, message: impl AsRef<str>) {
    note(Level::Warn, area, message)
}

pub fn error(area: &str, message: impl AsRef<str>) {
    note(Level::Error, area, message)
}

/// Point the shared book at a directory. Called once, when the app knows where
/// its config lives.
pub fn open_in(dir: &Path) {
    if let Ok(mut b) = book().lock() {
        b.open_in(dir);
    }
}

/// The last `n` lines of the shared book.
pub fn tail(n: usize) -> Vec<String> {
    book().lock().map(|b| b.tail(n)).unwrap_or_default()
}

/// Where the shared book is being written.
pub fn file() -> Option<PathBuf> {
    book()
        .lock()
        .ok()
        .and_then(|b| b.file().map(Path::to_path_buf))
}

/// Everything a maintainer needs to read a problem that happened on somebody
/// else's machine.
///
/// Pure apart from reading the book, so what the report *says* is testable
/// without running Flyway or starting an app. `probe` is the live half —
/// passed in rather than called here, because this module must not know what
/// Flyway is.
pub fn report(probe: &str, schema: &str, tabs: &str) -> String {
    let mut out = format!(
        "db-query \u{2014} diagnostics\n\
         (paths only, no passwords \u{2014} redact if you like)\n\n\
         app        {}\nos         {} {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    // **Whether this build updates itself at all**, which is the first thing to
    // rule out when somebody reports that a release did not reach them. It is
    // a property of the package, not of the network, and it is invisible from
    // the outside: a `.deb` simply never shows an update button.
    out.push_str(if crate::update::supported() {
        "updates    this build can update itself\n"
    } else {
        "updates    NOT this build \u{2014} install the newer .deb by hand\n"
    });
    // Named so a longer history than the ring holds can be attached rather
    // than copied — the file keeps far more than the 2000 lines below.
    out.push_str(&match file() {
        Some(p) => format!("log        {}\n\n", p.display()),
        None => "log        memory only \u{2014} nothing is being written\n\n".to_string(),
    });

    // **Before Flyway and before the log**, because this is the section that
    // answers the question that keeps being asked: the editor completes
    // nothing, or the linter says a table does not exist, and neither the
    // editor nor the tree can show why. Which database a tab is on, and how
    // much of it has been introspected, is the whole answer and none of it is
    // visible from the outside.
    out.push_str("=== schema ===\n");
    out.push_str(schema);
    out.push_str(tabs);

    out.push_str("\n=== Flyway ===\n");
    out.push_str(probe);

    let lines = tail(CAPACITY);
    out.push_str(&format!("\n=== log ({} lines) ===\n", lines.len()));
    if lines.is_empty() {
        out.push_str("(nothing recorded yet)\n");
    } else {
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("db-query-log-{tag}-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ------------------------------------------------------------ the ring

    #[test]
    fn the_ring_keeps_the_newest_and_drops_the_oldest() {
        let mut b = Logbook::new();
        for i in 0..CAPACITY + 50 {
            b.note(Level::Info, "app", &format!("entry {i}"));
        }
        assert_eq!(b.len(), CAPACITY, "the cap is a cap");

        let lines = b.lines();
        assert!(
            lines[0].contains("entry 50"),
            "the first fifty were evicted: {}",
            lines[0]
        );
        assert!(lines
            .last()
            .unwrap()
            .contains(&format!("entry {}", CAPACITY + 49)));
    }

    /// The boundary itself, because off-by-one is the whole risk in a ring.
    #[test]
    fn exactly_capacity_evicts_nothing() {
        let mut b = Logbook::new();
        for i in 0..CAPACITY {
            b.note(Level::Info, "app", &format!("entry {i}"));
        }
        assert_eq!(b.len(), CAPACITY);
        assert!(
            b.lines()[0].contains("entry 0"),
            "nothing should have gone yet"
        );

        b.note(Level::Info, "app", "one more");
        assert_eq!(b.len(), CAPACITY);
        assert!(b.lines()[0].contains("entry 1"), "now exactly one has gone");
    }

    #[test]
    fn a_line_reads_as_time_level_area_message() {
        let mut b = Logbook::new();
        b.note(Level::Warn, "flyway", "not found");
        let line = &b.lines()[0];
        assert!(line.contains("warn"), "{line}");
        assert!(line.contains("flyway"), "{line}");
        assert!(line.ends_with("not found"), "{line}");
        // `2026-09-16 14:03:11Z` — UTC, and said so.
        assert!(line.contains('Z'), "{line}");
        assert_eq!(&line[4..5], "-", "a date first, so it sorts: {line}");
    }

    #[test]
    fn the_tail_is_the_newest_n_in_order() {
        let mut b = Logbook::new();
        for i in 0..10 {
            b.note(Level::Info, "app", &format!("entry {i}"));
        }
        let tail = b.tail(3);
        assert_eq!(tail.len(), 3);
        assert!(tail[0].ends_with("entry 7"), "{:?}", tail);
        assert!(tail[2].ends_with("entry 9"), "{:?}", tail);
        // Asking for more than there is gives what there is.
        assert_eq!(b.tail(999).len(), 10);
    }

    // -------------------------------------------------------- the scrubber

    /// The ways a secret actually reaches a log. Every one of these is a
    /// mistake a call site could make, which is the reason the scrubber is
    /// here at all.
    #[test]
    fn a_credential_is_withheld_rather_than_edited() {
        let carriers = [
            "connecting to mysql://root:hunter2@db.internal:3306/app",
            "url = jdbc:mysql://svc:s3cr3t@10.0.0.4/orders",
            "ran CREATE USER 'bob'@'%' IDENTIFIED BY 'letmein'",
            "ALTER USER x IDENTIFIED WITH caching_sha2_password BY 'pw'",
            "sent header Authorization: Bearer sk-ant-api03-abcdef",
            "password=hunter2",
            "password: hunter2",
            "api_key=sk-12345",
            "token: ghp_aaaaaaaa",
        ];
        for c in carriers {
            assert!(looks_like_a_secret(c), "should have been caught: {c}");
            let mut b = Logbook::new();
            b.note(Level::Error, "mysql", c);
            let line = &b.lines()[0];
            assert!(line.contains("withheld"), "{line}");
            assert!(!line.contains("hunter2"), "{line}");
            assert!(!line.contains("s3cr3t"), "{line}");
            assert!(!line.contains("letmein"), "{line}");
        }
    }

    /// **The half that matters just as much.** A scrubber that eats the useful
    /// lines gets turned off, and then it protects nothing. Every one of these
    /// mentions a credential and carries no value.
    #[test]
    fn the_useful_lines_survive() {
        let keepers = [
            // The single most useful line MySQL ever prints.
            "Access denied for user 'root'@'localhost' (using password: YES)",
            "Access denied for user 'app'@'%' (using password: NO)",
            "connecting to mysql://root@db.internal:3306/app",
            "no password is stored for this connection",
            "the keychain refused: password: <none>",
            "password: ***",
            "flyway.cmd not found on the PATH",
            "GET https://es.internal:9200/_cat/indices returned 200",
            "connect failed for user 'root' at 10.0.0.4:3306",
        ];
        for k in keepers {
            assert!(!looks_like_a_secret(k), "should have survived: {k}");
            let mut b = Logbook::new();
            b.note(Level::Info, "mysql", k);
            assert!(b.lines()[0].ends_with(k), "{}", b.lines()[0]);
        }
    }

    /// `host:port` is a colon in an authority and is not a password. Getting
    /// this wrong would withhold every URL the app ever logs.
    #[test]
    fn a_port_is_not_a_password() {
        assert!(!looks_like_a_secret("http://127.0.0.1:9200/_search"));
        assert!(!looks_like_a_secret("listening on 127.0.0.1:49731"));
        // And a colon after the path is not userinfo either.
        assert!(!looks_like_a_secret("https://host/a:b@c"));
    }

    // ------------------------------------------------------------- the file

    #[test]
    fn entries_reach_the_file_and_survive_a_new_book() {
        let dir = tmp("file");
        let mut b = Logbook::new();
        b.open_in(&dir);
        b.note(Level::Info, "app", "started");
        b.note(Level::Error, "flyway", "not found");

        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("started"), "{text}");
        assert!(text.contains("not found"), "{text}");
        assert_eq!(text.lines().count(), 2);

        // A second book appends rather than truncating: a restart must not
        // throw away what the run before it recorded.
        let mut again = Logbook::new();
        again.open_in(&dir);
        again.note(Level::Info, "app", "started again");
        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert_eq!(text.lines().count(), 3, "{text}");
    }

    /// A withheld entry is withheld on disk too — which is the copy that
    /// actually persists.
    #[test]
    fn nothing_withheld_reaches_the_file() {
        let dir = tmp("secret");
        let mut b = Logbook::new();
        b.open_in(&dir);
        b.note(Level::Info, "mysql", "mysql://root:hunter2@db:3306/app");

        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("withheld"), "{text}");
    }

    #[test]
    fn the_file_rotates_once_it_is_too_big_and_keeps_one_predecessor() {
        let dir = tmp("rotate");
        // Faster than writing a megabyte an entry at a time, and it is the
        // size that triggers the rule, not how the size was reached.
        fs::write(dir.join(FILE_NAME), vec![b'x'; ROTATE_AT as usize + 1]).unwrap();

        let mut b = Logbook::new();
        b.open_in(&dir);
        b.note(Level::Info, "app", "after the rotation");

        let current = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(current.contains("after the rotation"));
        assert_eq!(current.lines().count(), 1, "the new file starts empty");

        let previous = dir.join(PREVIOUS_FILE_NAME);
        assert!(previous.is_file(), "the old one is kept, once");
        assert_eq!(fs::metadata(&previous).unwrap().len(), ROTATE_AT + 1);
    }

    // ----------------------------------------------------------- the report

    /// The report is what a user pastes, so what it *says* matters as much as
    /// that it exists. The global book is shared between tests, so this asserts
    /// only on the parts that are this test's own.
    #[test]
    fn the_report_names_the_build_the_platform_and_both_sections() {
        let text = report(
            "setting    \"flyway\"\nexit       0\n",
            "connection c1\n  poc: 12 tables named, 12 with columns cached (budget 60)\n",
            "tab t1  db poc\n",
        );

        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        assert!(text.contains(std::env::consts::OS), "{text}");
        // The warning has to be at the top, where somebody decides whether to
        // paste it, not at the bottom where they find out afterwards.
        let header: String = text.lines().take(3).collect::<Vec<_>>().join("\n");
        assert!(header.contains("no passwords"), "{header}");

        assert!(text.contains("=== Flyway ==="), "{text}");
        assert!(
            text.contains("exit       0"),
            "the probe is included verbatim"
        );
        assert!(text.contains("=== log ("), "{text}");
        // Where the longer history is, so it can be attached rather than
        // copied — the ring is 2000 lines and the file holds far more.
        assert!(text.contains("log        "), "{text}");

        // And whether this build updates itself at all. A `.deb` never shows
        // an update button, which from the outside is indistinguishable from
        // a release that never went out — the first thing to rule out, so it
        // is stated rather than inferred.
        assert!(text.contains("updates    "), "{text}");

        // The section that answers "why does it not know my tables?" — which
        // database a tab is on, and how much of it has been introspected.
        // Neither is visible from the editor, and the question always arrives
        // from somebody else's machine.
        assert!(text.contains("=== schema ==="), "{text}");
        assert!(text.contains("12 with columns cached"), "{text}");
        assert!(text.contains("tab t1  db poc"), "{text}");
        // Before the log, where somebody reading top-down finds it.
        assert!(
            text.find("=== schema ===") < text.find("=== log ("),
            "{text}"
        );
        assert_eq!(
            text.contains("NOT this build"),
            !crate::update::supported(),
            "the report must not disagree with the build it came from: {text}"
        );
    }

    /// First run: nothing has written to the config directory yet, so it does
    /// not exist. The log has to make it rather than quietly fail for the one
    /// person most likely to need it.
    #[test]
    fn a_config_directory_that_does_not_exist_yet_is_created() {
        let dir = tmp("fresh").join("never-made");
        assert!(!dir.exists());

        let mut b = Logbook::new();
        b.open_in(&dir);
        b.note(Level::Info, "app", "first run");

        assert_eq!(b.write_failures, 0);
        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("first run"), "{text}");
    }

    /// A log that cannot be written must not be able to stop anything.
    ///
    /// **The obstacle is a file where a directory has to go.** This once used
    /// an absurd absolute path, `/definitely/not/a/directory/that/exists`,
    /// which stopped meaning anything the moment `open_in` learned to create
    /// its directory: on Linux the test kept passing because nobody may
    /// `mkdir` in `/`, which is a fact about root permissions and not about
    /// this code. On Windows the same path is drive-relative, the directory
    /// was created happily, the write succeeded — and the test failed while
    /// littering `C:\` on the way.
    ///
    /// A regular file cannot be a directory on any platform, so this obstacle
    /// is the same everywhere, and it stays inside the temporary directory.
    #[test]
    fn an_unwritable_file_is_counted_not_raised() {
        let blocker = tmp("blocked").join("in-the-way");
        fs::write(&blocker, b"not a directory").unwrap();

        let mut b = Logbook::new();
        b.open_in(&blocker);
        b.note(Level::Info, "app", "still fine");

        assert_eq!(b.write_failures, 1, "the write failed and was counted");
        // And the entry is still in memory, which is where the button reads.
        assert_eq!(b.len(), 1);
        assert!(b.lines()[0].ends_with("still fine"));
        // The obstacle is untouched: nothing clobbered it trying to get past.
        assert_eq!(fs::read_to_string(&blocker).unwrap(), "not a directory");
    }
}
