//! The user's preferences, remembered beside their connections.
//!
//! # Why these are not left in the webview's storage
//!
//! They were, until it was reported that enabling an agent CLI never stuck.
//! The setting was being written and read back perfectly well — the store it
//! went into is simply **per webview origin**, and the app has more than one:
//!
//! | how it is run | origin | store |
//! |---|---|---|
//! | `mise run dev` | `http://localhost:1420` | `http_localhost_1420.localstorage` |
//! | the installed app | `tauri://localhost` | `tauri_localhost_0.localstorage` |
//!
//! Measured on Linux/WebKitGTK on 2026-10-03: both files exist under
//! `~/.local/share/com.dbquery.poc/localstorage/`, and the packaged build's
//! held nothing but a theme. So a preference chosen in one build is invisible
//! to the other, and a webview data directory cleared by an installer or a
//! WebView2 reset takes every preference with it.
//!
//! Connections, tabs and history never had that problem, because all three
//! live in the app config directory — see [`crate::workspace`], whose module
//! docs already give "kept out of the webview's own storage" as a property
//! worth having. This file is preferences joining them, so that everything the
//! app remembers is remembered in one place and by one rule.
//!
//! # What Rust does with the contents
//!
//! Nothing. The shape belongs to the frontend (`src/settings.ts`), which
//! validates every field on the way in because the value may have been written
//! by another version; re-declaring those fields here would be a second
//! description of one thing, and the two would drift. So the payload is stored
//! as the JSON object it is, and only the envelope is ours.
//!
//! **No secrets are in it**, by the same rule as everywhere else: API keys and
//! the MCP token are in the keychain, and a password is in the keychain. The
//! file is still restricted to the owner, like its neighbours — a preference
//! list names servers, paths and the tools someone has installed, which is not
//! a secret but is nobody else's business either.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const FILE_NAME: &str = "preferences.json";
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsStore {
    #[serde(default)]
    pub version: u32,
    /// The frontend's settings object, verbatim.
    #[serde(default)]
    pub settings: Map<String, Value>,
}

/// What reading the file produced.
///
/// `settings` is `None` when there is nothing stored — which is not an error
/// and is what tells the frontend to offer up whatever its own storage still
/// holds, so the first launch after this change keeps the choices already made.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadOutcome {
    pub settings: Option<Map<String, Value>>,
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

pub fn load(dir: &Path) -> LoadOutcome {
    let path = path_in(dir);
    if !path.exists() {
        return LoadOutcome {
            settings: None,
            warning: None,
        };
    }

    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return LoadOutcome {
                settings: None,
                warning: Some(format!("Could not read your preferences: {e}")),
            }
        }
    };

    match serde_json::from_str::<PrefsStore>(&text) {
        Ok(store) => LoadOutcome {
            settings: Some(store.settings),
            warning: None,
        },
        // Kept, not deleted. Preferences are reproducible — unlike an unsaved
        // buffer — but a file somebody may have hand-edited is still theirs,
        // and moving it aside is how the next save gets a clean one without
        // throwing away what was there.
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let aside = dir.join(format!("{FILE_NAME}.corrupt-{stamp}"));
            let moved = fs::rename(&path, &aside).is_ok();
            LoadOutcome {
                settings: None,
                warning: Some(if moved {
                    format!(
                        "Your preferences could not be read ({e}). The file was kept as {} \
                         and the app started on its defaults.",
                        aside.display()
                    )
                } else {
                    format!("Your preferences could not be read ({e}).")
                }),
            }
        }
    }
}

pub fn save(dir: &Path, settings: &Map<String, Value>) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the config directory: {e}"))?;
    let path = path_in(dir);

    // Create and restrict before writing anything into it.
    if !path.exists() {
        fs::File::create(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    }
    restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;

    let store = PrefsStore {
        version: FILE_VERSION,
        settings: settings.clone(),
    };
    let json = serde_json::to_string_pretty(&store)
        .map_err(|e| format!("Cannot serialise preferences: {e}"))?;
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
        let dir = std::env::temp_dir().join(format!("db-query-prefs-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn settings(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// **Nothing stored is not an error.** It is the signal that the frontend
    /// should hand over whatever its own storage holds, which is what carries
    /// an existing installation's choices into the file.
    #[test]
    fn an_absent_file_is_not_a_failure() {
        let out = load(&tmp());
        assert!(out.settings.is_none());
        assert!(out.warning.is_none());
    }

    /// Rust does not interpret the payload, so the test does not either: a
    /// field nothing here has ever heard of has to come back unchanged.
    #[test]
    fn whatever_went_in_comes_back_out() {
        let dir = tmp();
        let stored = settings(
            r#"{"cliEnabled":["copilot","claude"],"aiRecipe":"copilot",
                "fontSize":12,"somethingFromAFutureVersion":{"deep":[1,2,3]}}"#,
        );
        save(&dir, &stored).unwrap();
        assert_eq!(load(&dir).settings.unwrap(), stored);
    }

    /// The point of the whole module: a second read, in a second process, in a
    /// different webview, gets what the first one wrote.
    #[test]
    fn a_later_save_replaces_the_earlier_one() {
        let dir = tmp();
        save(&dir, &settings(r#"{"cliEnabled":[]}"#)).unwrap();
        save(&dir, &settings(r#"{"cliEnabled":["copilot"]}"#)).unwrap();
        assert_eq!(
            load(&dir).settings.unwrap(),
            settings(r#"{"cliEnabled":["copilot"]}"#)
        );
    }

    /// A file that cannot be parsed is kept, and says so. Deleting it would
    /// throw away a hand-edited file without telling anybody.
    #[test]
    fn an_unreadable_file_is_kept_and_reported() {
        let dir = tmp();
        fs::write(path_in(&dir), "{ this is not json").unwrap();

        let out = load(&dir);
        assert!(out.settings.is_none());
        let warning = out.warning.expect("a warning");
        assert!(warning.contains("corrupt-"), "{warning}");
        assert!(!path_in(&dir).exists(), "the bad file is out of the way");
        assert_eq!(
            fs::read_dir(&dir)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("corrupt-"))
                .count(),
            1,
            "and still on disk"
        );
    }

    /// An old file with no `version` still loads. The envelope is ours, so it
    /// is the one thing here that can gain fields, and a stored file written
    /// before one existed must not be read as corrupt.
    #[test]
    fn a_file_missing_the_envelope_fields_still_loads() {
        let dir = tmp();
        fs::write(path_in(&dir), r#"{"settings":{"fontSize":14}}"#).unwrap();
        assert_eq!(load(&dir).settings.unwrap(), settings(r#"{"fontSize":14}"#));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp();
        save(&dir, &settings(r#"{"fontSize":12}"#)).unwrap();
        let mode = fs::metadata(path_in(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
