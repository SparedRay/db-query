//! A reading of the workload counters, kept so a later one can be subtracted.
//!
//! # Why this exists
//!
//! `events_statements_summary_by_digest` is **cumulative and carries no
//! timestamps**. One reading is everything the server has done since it
//! started, which on the instance this was built against is fifty-four days of
//! mixed traffic. Two readings subtracted is a window, and a window is the only
//! way to ask "what did *my* change do".
//!
//! # Why a whole-file rewrite rather than an append
//!
//! A baseline is a **snapshot, not a log**: taking a new one replaces the old,
//! and there is no history to accumulate. That is `profiles.rs` and `prefs.rs`,
//! not `history.rs`. The file is rewritten when somebody presses a button, not
//! on every query, so the cost that made history append-only does not apply.
//!
//! # What it deliberately does not do
//!
//! It does not sample in the background. db-query is not a daemon, and a
//! baseline is taken when asked. A window you opened on Tuesday can still be
//! read on Friday; one you never opened does not exist.
//!
//! # Counters can go backwards
//!
//! A restart or a `TRUNCATE` resets them. Subtracting then yields negative
//! numbers, and a panel showing negative executions is worse than one showing
//! none — so [`subtract`] detects it and says the baseline is void rather than
//! reporting nonsense. See [`Delta::reset`].

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workload::DigestRow;

pub const FILE_NAME: &str = "baselines.json";

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// The subset of a digest row that is worth subtracting.
///
/// Not the whole row: `DIGEST_TEXT`, `FIRST_SEEN` and the quantiles are not
/// counters and have no difference. Keeping only the sums is also what makes
/// ten thousand of them a file rather than a problem.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Counters {
    pub execs: u64,
    pub total_picos: u64,
    pub lock_picos: u64,
    pub errors: u64,
    pub rows_sent: u64,
    pub rows_examined: u64,
    pub no_index: u64,
    pub scans: u64,
    pub sort_scans: u64,
    pub tmp_disk: u64,
}

impl Counters {
    fn of(row: &DigestRow) -> Self {
        Self {
            execs: row.execs,
            total_picos: row.total_picos,
            lock_picos: row.lock_picos,
            errors: row.errors,
            rows_sent: row.rows_sent,
            rows_examined: row.rows_examined,
            no_index: row.no_index,
            scans: row.scans,
            sort_scans: row.sort_scans,
            tmp_disk: row.tmp_disk,
        }
    }
}

/// One connection's reading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    /// Unix seconds. For telling the person how wide their window is.
    pub taken_at: i64,
    /// Digest to its counters at that moment.
    pub rows: HashMap<String, Counters>,
}

impl Baseline {
    pub fn of(rows: &[DigestRow]) -> Self {
        Self {
            taken_at: now(),
            rows: rows
                .iter()
                .map(|r| (r.digest.clone(), Counters::of(r)))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    /// Connection id to its baseline. One per server, so two servers do not
    /// share a window.
    #[serde(default)]
    baselines: HashMap<String, Baseline>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What a window looks like once the baseline has been taken off.
#[derive(Debug, Clone, Default)]
pub struct Delta {
    /// The same shapes, with their counters reduced to the window.
    ///
    /// A shape whose counters did not move at all is **dropped**: it did not
    /// run inside the window, and a list of zeroes is not a workload.
    pub rows: Vec<DigestRow>,
    /// Digests absent from the baseline, so never executed on this server
    /// before the window opened.
    ///
    /// The one attribution that survives a shared server: everything else in
    /// the window might be somebody else's traffic, but a shape that did not
    /// exist is new, and a new shape is usually a code change.
    pub new_shapes: Vec<String>,
    /// A counter went backwards, so the server restarted or the table was
    /// truncated and the baseline describes a different lifetime.
    pub reset: bool,
}

/// Take `base` off `current`.
///
/// Pure, so the arithmetic and the reset detection are testable without a
/// server — which matters, because the failure mode being guarded against only
/// happens when a server restarts.
pub fn subtract(current: &[DigestRow], base: &Baseline) -> Delta {
    // Any counter lower than it was means these are not the same counters.
    // Checked across everything before anything is subtracted: a reset found
    // halfway through would otherwise have already produced a dozen rows of
    // wrapped-around nonsense.
    let reset = current.iter().any(|row| {
        base.rows
            .get(&row.digest)
            .is_some_and(|was| row.execs < was.execs || row.total_picos < was.total_picos)
    });
    if reset {
        return Delta {
            rows: current.to_vec(),
            new_shapes: Vec::new(),
            reset: true,
        };
    }

    let mut rows: Vec<DigestRow> = Vec::new();
    let mut new_shapes: Vec<String> = Vec::new();

    for row in current {
        match base.rows.get(&row.digest) {
            None => {
                new_shapes.push(row.digest.clone());
                rows.push(row.clone());
            }
            Some(was) => {
                if row.execs == was.execs {
                    continue;
                }
                let mut moved = row.clone();
                moved.execs = row.execs - was.execs;
                moved.total_picos = row.total_picos.saturating_sub(was.total_picos);
                moved.lock_picos = row.lock_picos.saturating_sub(was.lock_picos);
                moved.errors = row.errors.saturating_sub(was.errors);
                moved.rows_sent = row.rows_sent.saturating_sub(was.rows_sent);
                moved.rows_examined = row.rows_examined.saturating_sub(was.rows_examined);
                moved.no_index = row.no_index.saturating_sub(was.no_index);
                moved.scans = row.scans.saturating_sub(was.scans);
                moved.sort_scans = row.sort_scans.saturating_sub(was.sort_scans);
                moved.tmp_disk = row.tmp_disk.saturating_sub(was.tmp_disk);
                // The average over the window, not the lifetime average the
                // server reports. Recomputed rather than carried: the whole
                // point of a window is that the lifetime figure is wrong.
                moved.avg_picos = moved.total_picos.checked_div(moved.execs).unwrap_or(0);
                // A quantile cannot be subtracted. Dropped rather than left
                // showing a lifetime p95 beside a window's average.
                moved.quantile_95_picos = None;
                rows.push(moved);
            }
        }
    }

    Delta {
        rows,
        new_shapes,
        reset: false,
    }
}

fn read(dir: &Path) -> Store {
    let path = path_in(dir);
    let Ok(text) = fs::read_to_string(&path) else {
        return Store::default();
    };
    match serde_json::from_str::<Store>(&text) {
        Ok(store) => store,
        // Moved aside rather than deleted, as `prefs.rs` does. A baseline is
        // reproducible — press the button again — but a file somebody may have
        // looked at is still theirs, and renaming it is how the next write
        // gets a clean one without destroying what was there.
        Err(_) => {
            let aside = dir.join(format!("{FILE_NAME}.corrupt-{}", now()));
            let _ = fs::rename(&path, &aside);
            Store::default()
        }
    }
}

fn write(dir: &Path, store: &Store) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let path = path_in(dir);
    let text = serde_json::to_string(store).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// This connection's baseline, if one was taken.
pub fn load(dir: &Path, connection_id: &str) -> Option<Baseline> {
    read(dir).baselines.remove(connection_id)
}

pub fn save(dir: &Path, connection_id: &str, baseline: &Baseline) -> Result<(), String> {
    let mut store = read(dir);
    store
        .baselines
        .insert(connection_id.to_string(), baseline.clone());
    write(dir, &store)
}

pub fn clear(dir: &Path, connection_id: &str) -> Result<(), String> {
    let mut store = read(dir);
    if store.baselines.remove(connection_id).is_none() {
        return Ok(());
    }
    write(dir, &store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(digest: &str, execs: u64, picos: u64, examined: u64) -> DigestRow {
        DigestRow {
            digest: digest.into(),
            text: format!("SELECT {digest}"),
            execs,
            total_picos: picos,
            avg_picos: picos.checked_div(execs).unwrap_or(0),
            rows_examined: examined,
            rows_sent: execs,
            ..Default::default()
        }
    }

    #[test]
    fn a_window_is_the_difference() {
        let base = Baseline::of(&[row("a", 100, 1_000, 5_000)]);
        let now = vec![row("a", 150, 1_600, 9_000)];
        let d = subtract(&now, &base);

        assert!(!d.reset);
        assert_eq!(d.rows.len(), 1);
        assert_eq!(d.rows[0].execs, 50);
        assert_eq!(d.rows[0].total_picos, 600);
        assert_eq!(d.rows[0].rows_examined, 4_000);
        assert_eq!(
            d.rows[0].avg_picos, 12,
            "the window's average, not the lifetime's"
        );
    }

    #[test]
    fn a_shape_that_did_not_run_is_dropped() {
        let base = Baseline::of(&[row("a", 100, 1_000, 5_000), row("b", 7, 70, 70)]);
        let now = vec![row("a", 101, 1_010, 5_050), row("b", 7, 70, 70)];
        let d = subtract(&now, &base);
        assert_eq!(d.rows.len(), 1, "a list of zeroes is not a workload");
        assert_eq!(d.rows[0].digest, "a");
    }

    #[test]
    fn a_shape_absent_from_the_baseline_is_new_and_kept_whole() {
        let base = Baseline::of(&[row("a", 100, 1_000, 5_000)]);
        let now = vec![row("a", 100, 1_000, 5_000), row("new", 3, 300, 900)];
        let d = subtract(&now, &base);

        assert_eq!(d.new_shapes, vec!["new"]);
        assert_eq!(d.rows.len(), 1);
        assert_eq!(d.rows[0].digest, "new");
        assert_eq!(d.rows[0].execs, 3, "all of it happened inside the window");
    }

    #[test]
    fn counters_going_backwards_voids_the_baseline() {
        // The server restarted. Subtracting would give negative executions,
        // and a panel of negative numbers is worse than no panel.
        let base = Baseline::of(&[row("a", 1_000, 10_000, 50_000)]);
        let now = vec![row("a", 12, 120, 600)];
        let d = subtract(&now, &base);

        assert!(d.reset);
        assert_eq!(d.rows[0].execs, 12, "the live reading, untouched");
        assert!(d.new_shapes.is_empty(), "nothing is 'new' across a reset");
    }

    #[test]
    fn a_reset_is_detected_before_any_row_is_subtracted() {
        // The reset is on the second shape. If the check were per row, the
        // first would already have been reported as a window.
        let base = Baseline::of(&[row("a", 10, 100, 100), row("b", 1_000, 10_000, 10_000)]);
        let now = vec![row("a", 20, 200, 200), row("b", 5, 50, 50)];
        let d = subtract(&now, &base);
        assert!(d.reset);
        assert_eq!(d.rows.len(), 2, "both live rows, neither subtracted");
        assert_eq!(d.rows[0].execs, 20);
    }

    #[test]
    fn a_quantile_is_dropped_rather_than_shown_beside_a_window() {
        let mut before = row("a", 100, 1_000, 1_000);
        before.quantile_95_picos = Some(9_999);
        let base = Baseline::of(&[before]);

        let mut after = row("a", 200, 2_000, 2_000);
        after.quantile_95_picos = Some(9_999);
        let d = subtract(&[after], &base);

        assert_eq!(
            d.rows[0].quantile_95_picos, None,
            "a lifetime p95 beside a window's average is a lie about both"
        );
    }

    #[test]
    fn an_empty_baseline_makes_everything_new() {
        let d = subtract(&[row("a", 5, 50, 50)], &Baseline::default());
        assert_eq!(d.new_shapes.len(), 1);
        assert!(!d.reset);
    }

    // ---- the file ------------------------------------------------------

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dbq-baseline-{}", uuid()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn uuid() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn a_baseline_survives_being_written_and_read() {
        let dir = tmp();
        let base = Baseline::of(&[row("a", 100, 1_000, 5_000)]);
        save(&dir, "c1", &base).unwrap();

        let back = load(&dir, "c1").expect("a baseline");
        assert_eq!(back.rows.len(), 1);
        assert_eq!(back.rows["a"].execs, 100);
        assert_eq!(back.taken_at, base.taken_at);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_connections_do_not_share_a_window() {
        let dir = tmp();
        save(&dir, "c1", &Baseline::of(&[row("a", 1, 1, 1)])).unwrap();
        save(&dir, "c2", &Baseline::of(&[row("a", 999, 1, 1)])).unwrap();

        assert_eq!(load(&dir, "c1").unwrap().rows["a"].execs, 1);
        assert_eq!(load(&dir, "c2").unwrap().rows["a"].execs, 999);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clearing_one_leaves_the_other() {
        let dir = tmp();
        save(&dir, "c1", &Baseline::of(&[row("a", 1, 1, 1)])).unwrap();
        save(&dir, "c2", &Baseline::of(&[row("a", 2, 2, 2)])).unwrap();
        clear(&dir, "c1").unwrap();

        assert!(load(&dir, "c1").is_none());
        assert!(load(&dir, "c2").is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clearing_a_baseline_nobody_took_is_not_an_error() {
        let dir = tmp();
        assert!(clear(&dir, "c1").is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_baseline_is_none_rather_than_a_failure() {
        let dir = tmp();
        assert!(load(&dir, "c1").is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_and_not_deleted() {
        let dir = tmp();
        fs::write(path_in(&dir), "{ this is not json").unwrap();

        assert!(load(&dir, "c1").is_none(), "started clean");
        let kept: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt"))
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "the file somebody may have looked at is theirs"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
