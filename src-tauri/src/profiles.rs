//! Saved connection profiles on disk.
//!
//! The file holds **no secrets, by construction**: [`ConnProfile`] has no
//! password field, so there is nothing to remember to strip. Passwords live in
//! the OS keychain (see [`crate::secrets`]).
//!
//! Three properties this module owes the user:
//!
//!   * **Versioned from day one.** A `version` field costs nothing now and
//!     removes the need to guess a format later.
//!   * **Atomic writes.** A half-written connection list would be worse than
//!     none, so writes go through [`crate::files::write_atomic`].
//!   * **A corrupt file is never silently discarded.** It is moved aside with a
//!     timestamped name and the app starts with an empty list, so the user can
//!     recover it — and can still open the app, which is what matters most.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::ConnProfile;

pub const FILE_NAME: &str = "connections.json";
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileStore {
    version: u32,
    profiles: Vec<ConnProfile>,
}

/// What reading the file produced. **Internal, not an IPC shape** — the command
/// layer converts these into [`crate::session::ProfileView`]s, which carry the
/// derived `rememberPassword` flag that must never be written to disk. Keeping
/// this type free of `Serialize` is what stops the two from being confused
/// again.
#[derive(Debug, Clone)]
pub struct LoadOutcome {
    pub profiles: Vec<ConnProfile>,
    /// Set when the file could not be read and was moved aside. The UI says so
    /// once, rather than pretending the user never had any connections.
    pub warning: Option<String>,
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Restrict the file to the owner. Called before any content is written, not
/// after — a window where the file exists world-readable is a window too many.
#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    // Windows and macOS place the app config dir under the user's profile,
    // which is already user-scoped by the OS.
    Ok(())
}

pub fn load(dir: &Path) -> LoadOutcome {
    let path = path_in(dir);
    if !path.exists() {
        return LoadOutcome {
            profiles: Vec::new(),
            warning: None,
        };
    }

    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return LoadOutcome {
                profiles: Vec::new(),
                warning: Some(format!("Could not read saved connections: {e}")),
            }
        }
    };

    match serde_json::from_str::<ProfileStore>(&text) {
        Ok(store) => LoadOutcome {
            profiles: store.profiles,
            warning: None,
        },
        Err(e) => {
            // Never delete it: the user may be able to fix it by hand, and a
            // list of servers is not something to lose quietly.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let aside = dir.join(format!("{FILE_NAME}.corrupt-{stamp}"));
            let moved = fs::rename(&path, &aside).is_ok();
            LoadOutcome {
                profiles: Vec::new(),
                warning: Some(if moved {
                    format!(
                        "Saved connections could not be read ({e}). The file was kept as {} \
                         and the list started empty.",
                        aside.display()
                    )
                } else {
                    format!("Saved connections could not be read ({e}).")
                }),
            }
        }
    }
}

pub fn save_all(dir: &Path, profiles: &[ConnProfile]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create the config directory: {e}"))?;
    let path = path_in(dir);

    // Create and restrict before writing anything into it.
    if !path.exists() {
        fs::File::create(&path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    }
    restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;

    let store = ProfileStore {
        version: FILE_VERSION,
        profiles: profiles.to_vec(),
    };
    let json = serde_json::to_string_pretty(&store)
        .map_err(|e| format!("Cannot serialise connections: {e}"))?;
    crate::files::write_atomic(&path, json.as_bytes())?;
    // write_atomic renames a fresh file over the target, so re-apply.
    restrict(&path)
        .map_err(|e| format!("Cannot restrict permissions on {}: {e}", path.display()))?;
    Ok(())
}

/// Insert or replace by id, preserving order for existing entries.
pub fn upsert(profiles: &mut Vec<ConnProfile>, profile: ConnProfile) {
    match profiles.iter_mut().find(|p| p.id == profile.id) {
        Some(slot) => *slot = profile,
        None => profiles.push(profile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "db-query-profiles-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(id: &str) -> ConnProfile {
        ConnProfile {
            id: id.into(),
            name: "prod-eu".into(),
            colour: "#ef4444".into(),
            host: "db.example.com".into(),
            port: 3306,
            user: "reporting".into(),
            database: Some("analytics".into()),
            allow_invalid_certs: false,
            kind: Default::default(),
            url: String::new(),
            auth: Default::default(),
            no_password: false,
        }
    }

    #[test]
    fn missing_file_is_an_empty_list_not_an_error() {
        let out = load(&tmpdir());
        assert!(out.profiles.is_empty());
        assert!(out.warning.is_none());
    }

    #[test]
    fn profiles_round_trip() {
        let d = tmpdir();
        save_all(&d, &[sample("a"), sample("b")]).unwrap();
        let out = load(&d);
        assert!(out.warning.is_none());
        assert_eq!(out.profiles.len(), 2);
        assert_eq!(out.profiles[0].id, "a");
        assert_eq!(out.profiles[0].name, "prod-eu");
        assert_eq!(out.profiles[0].database.as_deref(), Some("analytics"));
    }

    #[test]
    fn the_file_carries_a_version() {
        let d = tmpdir();
        save_all(&d, &[sample("a")]).unwrap();
        let raw = fs::read_to_string(path_in(&d)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["version"], 1, "no version field to migrate from later");
    }

    /// **C2's real check.** A secret must not be in the file, and cannot be even
    /// by accident, because `ConnProfile` has no field that holds one.
    ///
    /// This was a substring hunt for the words "password" and "secret". It fired
    /// on `noPassword` — a boolean saying an account *has* no password, which
    /// leaks nothing — and a word filter cannot tell that from a field that
    /// holds one. Weakening the filter would have been the wrong repair, so it
    /// is replaced by something stricter: **the exact set of keys written**.
    ///
    /// Any new field on `ConnProfile` now fails this until someone adds it to
    /// [`crate::session::PROFILE_KEYS`], which is a decision made on purpose
    /// rather than a word nobody happened to choose.
    #[test]
    fn the_file_contains_no_secret() {
        let d = tmpdir();
        save_all(&d, &[sample("a")]).unwrap();
        let raw = fs::read_to_string(path_in(&d)).unwrap();

        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut keys: Vec<&str> = v["profiles"][0]
            .as_object()
            .expect("a profile object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys[..],
            crate::session::PROFILE_KEYS[..],
            "a field appeared in the config file that nobody reviewed: {raw}"
        );

        // And no value that could be one, however a field is spelled.
        assert!(!raw.to_lowercase().contains("hunter2"), "{raw}");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir();
        save_all(&d, &[sample("a")]).unwrap();
        let mode = fs::metadata(path_in(&d)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got {mode:o}");
        // And still 0600 after a rewrite, since write_atomic renames over it.
        save_all(&d, &[sample("a"), sample("b")]).unwrap();
        let mode = fs::metadata(path_in(&d)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "permissions lost on rewrite: {mode:o}");
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_not_deleted() {
        let d = tmpdir();
        fs::write(path_in(&d), "{ this is not json").unwrap();

        let out = load(&d);
        assert!(out.profiles.is_empty(), "must not invent connections");
        let w = out.warning.expect("a corrupt file must be reported");
        assert!(w.contains("could not be read"), "{w}");

        // The original content survives under a new name.
        let kept: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1, "corrupt file was not preserved: {kept:?}");
        assert!(fs::read_to_string(d.join(&kept[0]))
            .unwrap()
            .contains("not json"));
    }

    #[test]
    fn upsert_replaces_by_id_and_keeps_order() {
        let mut v = vec![sample("a"), sample("b")];
        let mut edited = sample("a");
        edited.name = "renamed".into();
        upsert(&mut v, edited);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "renamed");
        assert_eq!(v[1].id, "b");

        upsert(&mut v, sample("c"));
        assert_eq!(v.len(), 3);
        assert_eq!(v[2].id, "c");
    }
}
