//! Where the window was, remembered across launches.
//!
//! Reported from use on Windows: every launch opened at the configured
//! 1280×800 in whatever place the OS chose, however the window had been left.
//!
//! # Why this is not a plugin
//!
//! `tauri-plugin-window-state` does exactly this. It is not used because
//! everything it needs is already in `tauri` itself — `outer_position`,
//! `inner_size`, `is_maximized`, `available_monitors`, `set_position`,
//! `set_size`, `maximize`, and the `Moved`/`Resized` window events (read from
//! the vendored `tauri-2.11.5/src/window/mod.rs` and
//! `tauri-runtime-2.11.3/src/window.rs` on 2026-10-13) — so the whole feature
//! is the fifty lines below plus one more file in the config directory, beside
//! `connections.json`, `session.json` and `preferences.json`. A new crate here
//! would be a new entry in the licence manifest for arithmetic.
//!
//! The one genuinely tricky part is deciding whether a remembered position is
//! still *reachable*, and that is the part worth owning and testing: a window
//! restored onto a monitor that has since been unplugged is a window nobody
//! can find, and it is the classic way this feature goes wrong.
//!
//! # What is stored, and when
//!
//! The **last non-maximized** geometry, plus whether the window was maximized
//! when it closed. Tauri does not expose a maximized window's restored rect,
//! so a window that is maximized at close would otherwise come back maximized
//! and then un-maximize to the size of the whole screen. Tracking the normal
//! geometry as the window moves costs a mutex write per event and no file I/O.
//!
//! The file is written **once, when the window is asked to close**. A crash or
//! a kill therefore loses the last move, which is the right trade: the
//! alternative is writing to disk while somebody drags a window.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "window.json";
const FILE_VERSION: u32 = 1;

/// How much of the window has to be on a monitor for its position to be
/// reused, in physical pixels, in both axes.
///
/// Enough to grab: a sliver of title bar you cannot hit with a pointer is the
/// same as no window at all.
const MIN_VISIBLE: i64 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    window: Option<Geometry>,
}

/// A monitor, as `available_monitors` describes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// What to do with a remembered geometry, given the monitors that exist now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub width: u32,
    pub height: u32,
    /// `None` means "leave the position to the OS" — the remembered one is
    /// somewhere nobody can reach.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
}

fn overlap(a0: i64, a_len: i64, b0: i64, b_len: i64) -> i64 {
    (a0 + a_len).min(b0 + b_len) - a0.max(b0)
}

/// Is enough of this rectangle on one of these monitors to be grabbed?
///
/// Each monitor is tested on its own rather than against their union: two
/// screens side by side leave no gap, but a window spanning the seam of an
/// L-shaped arrangement can be "inside the union" and over nothing.
fn reachable(g: &Rect, monitors: &[Rect]) -> bool {
    monitors.iter().any(|m| {
        if m.width == 0 || m.height == 0 {
            return false;
        }
        let w = overlap(g.x as i64, g.width as i64, m.x as i64, m.width as i64);
        let h = overlap(g.y as i64, g.height as i64, m.y as i64, m.height as i64);
        // A window smaller than the threshold only has to be fully on screen.
        let need_w = MIN_VISIBLE.min(g.width as i64);
        let need_h = MIN_VISIBLE.min(g.height as i64);
        w >= need_w && h >= need_h
    })
}

/// Decide what to apply.
///
/// Pure, because it is the part with the edge cases: a monitor that has gone
/// away, a window saved on a screen bigger than the one it reopens on, and an
/// empty monitor list (which is not a reason to refuse to place a window).
pub fn placement(saved: &Geometry, monitors: &[Rect]) -> Placement {
    let usable: Vec<&Rect> = monitors
        .iter()
        .filter(|m| m.width > 0 && m.height > 0)
        .collect();

    // No monitors reported is not a case to reason about — take the saved
    // values and let the window manager do what it likes with them.
    if usable.is_empty() {
        return Placement {
            width: saved.width,
            height: saved.height,
            position: Some((saved.x, saved.y)),
            maximized: saved.maximized,
        };
    }

    // Never larger than the biggest screen available now. A window remembered
    // from a 4K monitor must not open with its controls past the edge of a
    // laptop panel.
    let widest = usable.iter().map(|m| m.width).max().unwrap_or(saved.width);
    let tallest = usable
        .iter()
        .map(|m| m.height)
        .max()
        .unwrap_or(saved.height);
    let width = saved.width.min(widest);
    let height = saved.height.min(tallest);

    let owned: Vec<Rect> = usable.iter().map(|m| **m).collect();
    let rect = Rect {
        x: saved.x,
        y: saved.y,
        width,
        height,
    };
    Placement {
        width,
        height,
        position: reachable(&rect, &owned).then_some((saved.x, saved.y)),
        maximized: saved.maximized,
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Read the remembered geometry.
///
/// Anything unreadable is `None`. Unlike `preferences.json` there is no
/// warning and no file kept aside: a window size is not somebody's work, and
/// a message about it on launch would cost more attention than the thing is
/// worth. The next close overwrites it.
pub fn load(dir: &Path) -> Option<Geometry> {
    let text = fs::read_to_string(path_in(dir)).ok()?;
    serde_json::from_str::<WindowStore>(&text).ok()?.window
}

pub fn save(dir: &Path, geometry: &Geometry) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the config directory: {e}"))?;
    let store = WindowStore {
        version: FILE_VERSION,
        window: Some(*geometry),
    };
    let json =
        serde_json::to_string_pretty(&store).map_err(|e| format!("Cannot serialise: {e}"))?;
    crate::files::write_atomic(&path_in(dir), json.as_bytes())
}

// ------------------------------------------------------- the live window

/// The last geometry seen while the window was **not** maximized.
///
/// A `static`, like `flywaycli::LAST_RUN`: there is one main window, and
/// threading a managed struct through a window-event closure would be more
/// plumbing than the fact deserves.
static LAST_NORMAL: Mutex<Option<Geometry>> = Mutex::new(None);

/// Note where the window is, called on every move and resize.
///
/// Cheap on purpose — a mutex write, no file I/O — because this fires
/// continuously while a window is dragged.
pub fn remember(window: &tauri::Window) {
    // A maximized or fullscreen window's rect is the screen's, which is not
    // what we want to restore it *to*.
    if window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    // A minimized window reports a nonsense rect on some platforms; a zero
    // dimension is the tell, and there is nothing worth recording about it.
    if size.width == 0 || size.height == 0 {
        return;
    }
    if let Ok(mut last) = LAST_NORMAL.lock() {
        *last = Some(Geometry {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
            maximized: false,
        });
    }
}

/// Write what was last seen, plus whether the window is maximized now.
///
/// Called when the window is asked to close. If nothing was ever noted — the
/// window was never moved or resized — the current rect is read instead, so a
/// maximized-from-launch window is still recorded as maximized.
pub fn persist(window: &tauri::Window, dir: &Path) {
    let maximized = window.is_maximized().unwrap_or(false);
    let noted = LAST_NORMAL.lock().ok().and_then(|l| *l);
    let geometry = match noted {
        Some(g) => Geometry { maximized, ..g },
        None => {
            let Ok(pos) = window.outer_position() else {
                return;
            };
            let Ok(size) = window.inner_size() else {
                return;
            };
            Geometry {
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
                maximized,
            }
        }
    };
    if let Err(e) = save(dir, &geometry) {
        crate::logbook::warn("window", format!("could not remember the window: {e}"));
    }
}

/// Put the window back where it was. Does nothing if nothing is remembered.
pub fn restore(window: &tauri::Window, dir: &Path) {
    let Some(saved) = load(dir) else {
        return;
    };
    let monitors: Vec<Rect> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| Rect {
            x: m.position().x,
            y: m.position().y,
            width: m.size().width,
            height: m.size().height,
        })
        .collect();

    let want = placement(&saved, &monitors);
    // Size and position before maximizing: these are what un-maximizing will
    // reveal, and setting them afterwards would fight the maximize.
    let _ = window.set_size(tauri::PhysicalSize::new(want.width, want.height));
    if let Some((x, y)) = want.position {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    } else {
        crate::logbook::info(
            "window",
            format!(
                "the remembered position ({}, {}) is off every monitor; letting the OS place it",
                saved.x, saved.y
            ),
        );
    }
    if want.maximized {
        let _ = window.maximize();
    }
    // Seed the tracker, so closing without ever moving the window re-saves
    // what was restored rather than nothing.
    if let Ok(mut last) = LAST_NORMAL.lock() {
        *last = Some(Geometry {
            x: want.position.map(|p| p.0).unwrap_or(saved.x),
            y: want.position.map(|p| p.1).unwrap_or(saved.y),
            width: want.width,
            height: want.height,
            maximized: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    /// A second screen to the right, as a desktop reports one.
    const RIGHT: Rect = Rect {
        x: 1920,
        y: 0,
        width: 2560,
        height: 1440,
    };

    fn at(x: i32, y: i32, width: u32, height: u32) -> Geometry {
        Geometry {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    fn tmp() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("db-query-win-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The ordinary case: nothing has changed, so nothing is adjusted.
    #[test]
    fn a_window_on_a_screen_that_still_exists_is_put_back_exactly() {
        let want = placement(&at(100, 80, 1280, 800), &[LAPTOP]);
        assert_eq!(want.position, Some((100, 80)));
        assert_eq!((want.width, want.height), (1280, 800));
    }

    /// **The bug this guards.** The window was on the second screen; that
    /// screen is gone; its remembered position is over nothing.
    #[test]
    fn a_position_on_an_unplugged_monitor_is_dropped() {
        let on_second = at(2200, 300, 1280, 800);
        assert_eq!(
            placement(&on_second, &[LAPTOP, RIGHT]).position,
            Some((2200, 300))
        );
        // Same geometry, one screen now.
        let want = placement(&on_second, &[LAPTOP]);
        assert_eq!(want.position, None, "nobody could reach that window");
        // The size is still honoured — only the position was unreachable.
        assert_eq!((want.width, want.height), (1280, 800));
    }

    /// Mostly off the right-hand edge but still grabbable: kept. Dragging a
    /// window nearly off screen is something people do on purpose.
    #[test]
    fn a_window_hanging_off_an_edge_is_still_reachable() {
        let want = placement(&at(1840, 500, 1280, 800), &[LAPTOP]);
        assert_eq!(want.position, Some((1840, 500)));
    }

    /// One pixel past the threshold, in each axis in turn.
    #[test]
    fn the_visible_threshold_is_where_it_says_it_is() {
        // 80px of width left on screen: kept.
        assert!(placement(&at(1840, 100, 1280, 800), &[LAPTOP])
            .position
            .is_some());
        // 79px: dropped.
        assert!(placement(&at(1841, 100, 1280, 800), &[LAPTOP])
            .position
            .is_none());
        // And vertically, which is the case a lower taskbar produces.
        assert!(placement(&at(100, 1000, 1280, 800), &[LAPTOP])
            .position
            .is_some());
        assert!(placement(&at(100, 1001, 1280, 800), &[LAPTOP])
            .position
            .is_none());
    }

    /// Negative coordinates are normal — a screen above or to the left of the
    /// primary one — and must not be mistaken for being off screen.
    #[test]
    fn a_monitor_to_the_left_has_negative_coordinates_and_is_fine() {
        let left = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let want = placement(&at(-1800, 40, 1280, 800), &[left, LAPTOP]);
        assert_eq!(want.position, Some((-1800, 40)));
    }

    /// A window remembered from the big screen, reopened on the laptop alone.
    #[test]
    fn a_window_larger_than_every_screen_is_cut_down_to_fit() {
        let want = placement(&at(0, 0, 2560, 1400), &[LAPTOP]);
        assert_eq!((want.width, want.height), (1920, 1080));
    }

    /// The window spans the seam of two screens arranged in an L, so it is
    /// inside their bounding box and over neither of them. Tested per monitor
    /// for exactly this reason.
    #[test]
    fn a_window_in_the_gap_of_an_l_shaped_desktop_is_not_reachable() {
        let lower_left = Rect {
            x: 0,
            y: 1080,
            width: 800,
            height: 600,
        };
        // Far to the right of the lower screen and far below the upper one.
        let want = placement(&at(1500, 1300, 300, 200), &[LAPTOP, lower_left]);
        assert_eq!(want.position, None);
    }

    /// Nothing to reason against. Refusing to place the window would be worse
    /// than trusting what was stored.
    #[test]
    fn with_no_monitors_reported_the_saved_values_are_used_as_they_are() {
        let want = placement(&at(2200, 300, 1280, 800), &[]);
        assert_eq!(want.position, Some((2200, 300)));
        assert_eq!((want.width, want.height), (1280, 800));
    }

    /// A monitor the platform reports with no size tells us nothing, and must
    /// not be the reason a position is kept or a size is shrunk to zero.
    #[test]
    fn a_degenerate_monitor_is_ignored_rather_than_believed() {
        let nothing = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(
            placement(&at(10, 10, 1280, 800), &[nothing]).position,
            Some((10, 10))
        );
        assert_eq!(
            (placement(&at(10, 10, 1280, 800), &[nothing]).width, 1280).0,
            1280,
            "a zero-size monitor must not clamp the window to nothing"
        );
    }

    #[test]
    fn maximized_survives_the_round_trip() {
        let dir = tmp();
        let g = Geometry {
            maximized: true,
            ..at(10, 20, 1024, 768)
        };
        save(&dir, &g).unwrap();
        assert_eq!(load(&dir), Some(g));
        assert!(placement(&g, &[LAPTOP]).maximized);
    }

    #[test]
    fn nothing_stored_is_none_rather_than_a_default() {
        assert_eq!(load(&tmp()), None);
    }

    /// An unreadable file is not worth a message, so it reads as "nothing
    /// remembered" and the next close replaces it.
    #[test]
    fn an_unreadable_file_reads_as_nothing_remembered() {
        let dir = tmp();
        fs::write(path_in(&dir), "{ not json").unwrap();
        assert_eq!(load(&dir), None);
        // And saving over it works, so the app heals itself.
        save(&dir, &at(1, 2, 300, 400)).unwrap();
        assert_eq!(load(&dir), Some(at(1, 2, 300, 400)));
    }
}
