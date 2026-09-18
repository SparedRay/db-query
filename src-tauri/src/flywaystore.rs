//! The Flyway projects the user has added, on disk.
//!
//! Stage 17: a project stands on its own rather than being attached to a
//! connection, so it needs somewhere of its own to live. Same rules as
//! `connections.json`, for the same reasons:
//!
//!   * **Paths, never contents.** The TOML lives in a repository and changes
//!     with the branch; it is re-read every time it is used (Stage 15 §3.3).
//!   * **No secret, by construction.** Nothing here has a field that could hold
//!     one, and [`PROJECT_KEYS`] pins the exact key set so that adding a field
//!     means answering "is this safe to write to disk?".
//!   * **Atomic writes; a corrupt file is moved aside, never discarded.**

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "flyway_projects.json";
const FILE_VERSION: u32 = 1;

/// One added project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProject {
    pub id: String,
    /// The `flyway.toml`.
    pub path: String,
    /// The environment last selected. **A preference, not a safety fact**: every
    /// write re-reads the file and checks the target again, so a stale value
    /// here can only select the wrong row, never write to the wrong database.
    #[serde(default)]
    pub environment: Option<String>,
}

/// Every key a serialised [`StoredProject`] has, in sorted order. Asserted
/// against, exactly as `PROFILE_KEYS` is.
pub const PROJECT_KEYS: [&str; 3] = ["environment", "id", "path"];

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    projects: Vec<StoredProject>,
}

pub struct LoadOutcome {
    pub projects: Vec<StoredProject>,
    pub warning: Option<String>,
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// A fresh id. Unique within one run by the counter, across runs by the clock.
pub fn new_id() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("fp{millis:x}-{}", N.fetch_add(1, Ordering::SeqCst))
}

/// The projects, seeding the file from legacy attachments the first time.
pub fn load(dir: &Path) -> LoadOutcome {
    let path = path_in(dir);
    if !path.exists() {
        let seeded = legacy_attachments(dir);
        // Written straight away, so the seed happens once. If it cannot be
        // written the projects are still shown; the next add will try again.
        if !seeded.is_empty() {
            let _ = save_all(dir, &seeded);
        }
        return LoadOutcome {
            projects: seeded,
            warning: None,
        };
    }

    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return LoadOutcome {
                projects: Vec::new(),
                warning: Some(format!("Could not read Flyway projects: {e}")),
            }
        }
    };
    match serde_json::from_str::<Store>(&text) {
        Ok(store) => LoadOutcome {
            projects: store.projects,
            warning: None,
        },
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let aside = dir.join(format!("{FILE_NAME}.corrupt-{stamp}"));
            let moved = fs::rename(&path, &aside).is_ok();
            LoadOutcome {
                projects: Vec::new(),
                warning: Some(if moved {
                    format!(
                        "Flyway projects could not be read ({e}). The file was kept as {} \
                         and the list started empty.",
                        aside.display()
                    )
                } else {
                    format!("Flyway projects could not be read ({e}).")
                }),
            }
        }
    }
}

pub fn save_all(dir: &Path, projects: &[StoredProject]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the config directory: {e}"))?;
    let path = path_in(dir);
    if !path.exists() {
        fs::File::create(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    }
    crate::profiles::restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;

    let json = serde_json::to_string_pretty(&Store {
        version: FILE_VERSION,
        projects: projects.to_vec(),
    })
    .map_err(|e| format!("Cannot serialise Flyway projects: {e}"))?;
    crate::files::write_atomic(&path, json.as_bytes())?;
    crate::profiles::restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;
    Ok(())
}

/// Stage 15 kept a project on each connection profile. Those fields are gone
/// from `ConnProfile`, so they are read here from the raw JSON — **once**, to
/// turn each distinct project into an entry of its own (Stage 17 §3.5).
///
/// The same file attached to three connections becomes one project; the first
/// connection's environment becomes the selected one.
fn legacy_attachments(dir: &Path) -> Vec<StoredProject> {
    let Ok(text) = fs::read_to_string(crate::profiles::path_in(dir)) else {
        return Vec::new();
    };
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut out: Vec<StoredProject> = Vec::new();
    for p in raw
        .get("profiles")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
    {
        let Some(path) = p.get("flywayProject").and_then(|v| v.as_str()) else {
            continue;
        };
        if path.is_empty() || out.iter().any(|o| o.path == path) {
            continue;
        }
        out.push(StoredProject {
            id: new_id(),
            path: path.to_string(),
            environment: p
                .get("flywayEnvironment")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "db-query-flywaystore-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> StoredProject {
        StoredProject {
            id: new_id(),
            path: "/repo/flyway.toml".into(),
            environment: Some("uat".into()),
        }
    }

    #[test]
    fn projects_round_trip() {
        let dir = tmpdir();
        save_all(&dir, &[sample()]).unwrap();
        let back = load(&dir);
        assert!(back.warning.is_none());
        assert_eq!(back.projects.len(), 1);
        assert_eq!(back.projects[0].path, "/repo/flyway.toml");
        assert_eq!(back.projects[0].environment.as_deref(), Some("uat"));
    }

    /// F8. The file's keys are exactly these. A field added to
    /// `StoredProject` fails here until somebody decides it is safe on disk.
    #[test]
    fn the_file_holds_exactly_the_known_keys() {
        let dir = tmpdir();
        save_all(&dir, &[sample()]).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path_in(&dir)).unwrap()).unwrap();
        let mut keys: Vec<&str> = raw["projects"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(keys, PROJECT_KEYS);
    }

    /// F7. What Stage 15 attached to connections becomes projects, once, one
    /// per distinct file — and the passwords that file never had stay absent.
    #[test]
    fn legacy_attachments_become_projects_once() {
        let dir = tmpdir();
        fs::write(
            crate::profiles::path_in(&dir),
            r#"{"version":1,"profiles":[
                {"id":"a","name":"Dev","flywayProject":"/repo/flyway.toml","flywayEnvironment":"development"},
                {"id":"b","name":"QA","flywayProject":"/repo/flyway.toml","flywayEnvironment":"qa"},
                {"id":"c","name":"Other","flywayProject":"/other/flyway.toml","flywayEnvironment":null},
                {"id":"d","name":"None"}
            ]}"#,
        )
        .unwrap();

        let first = load(&dir);
        let paths: Vec<&str> = first.projects.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, ["/repo/flyway.toml", "/other/flyway.toml"]);
        assert_eq!(
            first.projects[0].environment.as_deref(),
            Some("development")
        );
        assert!(
            path_in(&dir).exists(),
            "the seed is written, so it happens once"
        );

        // Removing every project must not bring them back from the profiles.
        save_all(&dir, &[]).unwrap();
        assert!(load(&dir).projects.is_empty());
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_not_discarded() {
        let dir = tmpdir();
        fs::write(path_in(&dir), "{ not json").unwrap();
        let out = load(&dir);
        assert!(out.projects.is_empty());
        assert!(out.warning.unwrap().contains("kept as"));
        assert!(fs::read_dir(&dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".corrupt-")));
    }
}
