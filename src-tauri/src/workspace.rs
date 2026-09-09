//! The open tabs, remembered across restarts.
//!
//! Same five properties as [`crate::profiles`], for the same five reasons:
//! versioned from day one, written atomically, restricted to the owner, never
//! silently discarded when corrupt, and kept out of the webview's own storage.
//!
//! Two of them are load-bearing here in a way they are not for a connection
//! list:
//!
//!   * **This file can be the only copy of someone's work.** An untitled buffer
//!     exists nowhere else, so a half-written file is a lost afternoon.
//!   * **A buffer can contain a secret.** `CREATE USER … IDENTIFIED BY '…'` in
//!     an unsaved tab lives only in RAM today; storing it puts it on disk. Hence
//!     0600 and the app config directory, exactly like `connections.json`.
//!
//! Rust does not interpret any of this — the shape is the frontend's. It is
//! spelled out as types rather than kept as an opaque blob so that the format
//! is documented, versioned and testable in one place.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "session.json";
const FILE_VERSION: u32 = 1;

/// One tab, as it is written down.
///
/// Everything that describes a *result* is absent by design: results are
/// unbounded and stale by definition, and painting yesterday's rows as though
/// they were a result is worse than showing none.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTab {
    pub title: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub dialect: Option<String>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub line_ending: Option<String>,
    /// The mtime this tab's baseline came from, so the existing save-time
    /// conflict check still has something to compare against after a restart.
    #[serde(default)]
    pub mtime_ms: Option<f64>,
    /// The buffer — stored **only when it is the only copy**: an untitled tab,
    /// or a file-backed tab with unsaved edits. A clean file-backed tab stores
    /// its path and is re-read on restore, which keeps the common case to a few
    /// hundred bytes.
    #[serde(default)]
    pub text: Option<String>,
    /// Cursor offset. Clamped by the frontend, which is the only side that
    /// knows how long the document turned out to be.
    #[serde(default)]
    pub cursor: usize,
    #[serde(default)]
    pub active_db: Option<String>,
    #[serde(default)]
    pub untitled_number: Option<u32>,
    /// This tab arrived through the MCP server rather than being opened by the
    /// person at the keyboard.
    ///
    /// Stored, so the mark survives a restart. Provenance that lasts only until
    /// the app is closed is provenance you cannot rely on, and the whole reason
    /// the mark exists is that a tab which appeared unbidden must not look like
    /// one you opened. `#[serde(default)]` so every session file written before
    /// Stage 13 loads unchanged and means "mine".
    #[serde(default)]
    pub external: bool,
}

/// One connection's tabs. Keyed by profile id, because a tab belongs to a
/// connection for life and never migrates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredWorkspace {
    pub connection_id: String,
    #[serde(default)]
    pub tabs: Vec<StoredTab>,
    /// Which tab was in front. An index rather than an id: restored tabs are
    /// minted fresh ids, so a stored id would refer to nothing.
    #[serde(default)]
    pub active_index: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStore {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub connections: Vec<StoredWorkspace>,
}

/// What reading the file produced. A warning means the file was unreadable and
/// moved aside — the app still starts, which matters more.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadOutcome {
    pub session: SessionStore,
    pub warning: Option<String>,
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Restrict to the owner. Called before any content is written, not after.
#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn empty() -> SessionStore {
    SessionStore {
        version: FILE_VERSION,
        connections: Vec::new(),
    }
}

pub fn load(dir: &Path) -> LoadOutcome {
    let path = path_in(dir);
    if !path.exists() {
        return LoadOutcome {
            session: empty(),
            warning: None,
        };
    }

    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return LoadOutcome {
                session: empty(),
                warning: Some(format!("Could not read the saved session: {e}")),
            }
        }
    };

    match serde_json::from_str::<SessionStore>(&text) {
        Ok(session) => LoadOutcome {
            session,
            warning: None,
        },
        Err(e) => {
            // Never delete it. This file can hold the only copy of an unsaved
            // buffer, so it is kept even when it cannot be understood.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let aside = dir.join(format!("{FILE_NAME}.corrupt-{stamp}"));
            let moved = fs::rename(&path, &aside).is_ok();
            LoadOutcome {
                session: empty(),
                warning: Some(if moved {
                    format!(
                        "The saved session could not be read ({e}). The file was kept as {} \
                         and the app started with no restored tabs.",
                        aside.display()
                    )
                } else {
                    format!("The saved session could not be read ({e}).")
                }),
            }
        }
    }
}

pub fn save(dir: &Path, session: &SessionStore) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the config directory: {e}"))?;
    let path = path_in(dir);

    // Create and restrict before writing anything into it.
    if !path.exists() {
        fs::File::create(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    }
    restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;

    let store = SessionStore {
        version: FILE_VERSION,
        connections: session.connections.clone(),
    };
    let json = serde_json::to_string_pretty(&store)
        .map_err(|e| format!("Cannot serialise session: {e}"))?;
    crate::files::write_atomic(&path, json.as_bytes())?;
    // write_atomic renames a fresh file over the target, so re-apply.
    restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("db-query-ws-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tab(title: &str) -> StoredTab {
        StoredTab {
            title: title.into(),
            text: Some("SELECT 1;".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_missing_file_is_an_empty_session_not_an_error() {
        let out = load(&tmp());
        assert!(out.session.connections.is_empty());
        assert!(out.warning.is_none());
    }

    #[test]
    fn a_saved_session_round_trips() {
        let dir = tmp();
        let store = SessionStore {
            version: FILE_VERSION,
            connections: vec![StoredWorkspace {
                connection_id: "c1".into(),
                tabs: vec![tab("Untitled-1"), tab("report.sql")],
                active_index: 1,
            }],
        };
        save(&dir, &store).unwrap();

        let back = load(&dir);
        assert!(back.warning.is_none());
        assert_eq!(back.session.version, FILE_VERSION);
        assert_eq!(back.session.connections.len(), 1);
        let ws = &back.session.connections[0];
        assert_eq!(ws.connection_id, "c1");
        assert_eq!(ws.active_index, 1);
        assert_eq!(ws.tabs[1].title, "report.sql");
        assert_eq!(ws.tabs[0].text.as_deref(), Some("SELECT 1;"));
    }

    /// The version is stamped by `save`, never taken from the caller — so a
    /// frontend that forgets to set it cannot write an unversioned file.
    #[test]
    fn save_stamps_the_current_version() {
        let dir = tmp();
        save(
            &dir,
            &SessionStore {
                version: 999,
                connections: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(load(&dir).session.version, FILE_VERSION);
    }

    /// This file can be the only copy of an unsaved buffer, so a parse failure
    /// must never delete it.
    #[test]
    fn a_corrupt_file_is_kept_and_reported() {
        let dir = tmp();
        fs::write(path_in(&dir), b"{not json").unwrap();

        let out = load(&dir);
        assert!(out.session.connections.is_empty());
        let warning = out.warning.expect("a corrupt session must be reported");
        assert!(warning.contains("corrupt-"), "{warning}");

        let kept: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1, "the unreadable file must still exist");
    }

    /// Unknown fields from a newer version must not throw the file away — the
    /// tabs it does understand are still someone's work.
    #[test]
    fn an_unknown_field_does_not_discard_the_session() {
        let dir = tmp();
        fs::write(
            path_in(&dir),
            br#"{"version":1,"connections":[{"connectionId":"c1","tabs":[{"title":"a","somethingNew":true}],"activeIndex":0}]}"#,
        )
        .unwrap();

        let out = load(&dir);
        assert!(out.warning.is_none());
        assert_eq!(out.session.connections[0].tabs[0].title, "a");
    }

    /// A session file written before Stage 13 has no `external` key at all,
    /// and the answer for every tab in it is "the user opened this". Getting
    /// this wrong would put a provenance mark on everyone's existing tabs the
    /// first time they ran a new build.
    #[test]
    fn a_session_from_before_provenance_existed_means_the_tabs_are_yours() {
        let dir = tmp();
        fs::write(
            path_in(&dir),
            br#"{"version":1,"connections":[{"connectionId":"c1","tabs":[{"title":"a","text":"SELECT 1;"}],"activeIndex":0}]}"#,
        )
        .unwrap();

        let out = load(&dir);
        assert!(out.warning.is_none());
        assert!(!out.session.connections[0].tabs[0].external);
    }

    #[test]
    fn an_external_tab_stays_external_across_a_save_and_load() {
        let dir = tmp();
        let mut marked = tab("from-a-client");
        marked.external = true;
        save(
            &dir,
            &SessionStore {
                version: FILE_VERSION,
                connections: vec![StoredWorkspace {
                    connection_id: "c1".into(),
                    tabs: vec![tab("mine"), marked],
                    active_index: 0,
                }],
            },
        )
        .unwrap();

        let back = load(&dir);
        let tabs = &back.session.connections[0].tabs;
        assert!(!tabs[0].external, "an ordinary tab was marked");
        assert!(tabs[1].external, "the mark did not survive the round trip");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp();
        let store = SessionStore {
            version: FILE_VERSION,
            connections: vec![StoredWorkspace {
                connection_id: "c1".into(),
                tabs: vec![tab("secrets")],
                active_index: 0,
            }],
        };
        save(&dir, &store).unwrap();
        let mode = fs::metadata(path_in(&dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        // And still 0600 after a rewrite, since write_atomic renames over it.
        save(&dir, &store).unwrap();
        let mode = fs::metadata(path_in(&dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
