//! Running the Flyway command line, and reading what it says back.
//!
//! # This is the first process this app has ever started
//!
//! Everything else here talks to a server over a protocol. Spawning a program
//! is a different kind of thing, so three rules:
//!
//!   * **Arguments are a vector, never a shell string.** Nothing is
//!     interpolated into a command line, so a path with a space, a quote or a
//!     semicolon in it is an argument and not an instruction.
//!   * **The working directory is the project's folder.** Flyway resolves a
//!     relative `locations` against the process working directory, not against
//!     the config file — `filesystem:migrations` works for the user because
//!     they run Flyway from there. Replicating that is more honest than
//!     rewriting paths and hoping we handle every form.
//!   * **A missing binary is an ordinary answer, not a crash.** Most people
//!     will not have Flyway on their PATH, and the message has to say so and
//!     say where to fix it.
//!
//! `std::process::Command` on the blocking pool rather than `tokio::process`:
//! that feature pulls tokio's unix signal machinery, and this is one short
//! command run when somebody clicks a button — not something that needs to be
//! woven into the reactor.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// What to run when nobody has said otherwise: whatever is on the PATH.
pub const DEFAULT_PROGRAM: &str = "flyway";

/// A finished run. **Not** a success — Flyway reports failure through the exit
/// code *and* through the JSON, and both are kept so the caller can decide.
#[derive(Debug, Clone)]
pub struct Finished {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Finished {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Build the arguments for one operation. Split out from the spawning so the
/// **shape of the command line is testable without running anything.**
pub fn arguments(
    config_file_name: &str,
    environment: &str,
    op: &str,
    extra: &[String],
) -> Vec<String> {
    let mut args = vec![
        format!("-configFiles={config_file_name}"),
        format!("-environment={environment}"),
        // Every command this app runs is parsed, never shown raw.
        "-outputType=json".to_string(),
    ];
    args.extend(extra.iter().cloned());
    // The operation last: it is what the flags apply to.
    args.push(op.to_string());
    args
}

/// Run one Flyway operation against a project file.
///
/// `project` is the path to the `flyway.toml`; its folder becomes the working
/// directory and only its file name is passed to `-configFiles`, which is
/// exactly how a person runs it by hand.
pub async fn run(
    program: &str,
    project: &Path,
    environment: &str,
    op: &str,
    extra: Vec<String>,
) -> Result<Finished, String> {
    let dir: PathBuf = project
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no folder to run in.", project.display()))?;
    let file = project
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| format!("{} is not a readable file name.", project.display()))?
        .to_string();

    let args = arguments(&file, environment, op, &extra);
    let program = program.to_string();
    let shown = program.clone();

    tokio::task::spawn_blocking(move || {
        let out = Command::new(&program)
            .args(&args)
            .current_dir(&dir)
            // Flyway asks nothing interactively, but a program that decided to
            // would otherwise hang a click forever with no way to answer it.
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!(
                    "Flyway was not found at \"{shown}\". Install the Flyway command line, or \
                     set the path to it in Settings → Integrations."
                ),
                std::io::ErrorKind::PermissionDenied => {
                    format!("\"{shown}\" cannot be run: permission denied.")
                }
                _ => format!("Could not run \"{shown}\": {e}"),
            })?;
        Ok(Finished {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    })
    .await
    .map_err(|e| format!("The Flyway run could not be waited for: {e}"))?
}

// -------------------------------------------------------- reading the JSON

/// One migration, as `info` describes it.
///
/// Field names measured against Flyway 13.5.0 rather than taken from the
/// documentation, which was wrong about three of them — see §9 of the Stage 15
/// tracker. Everything is optional because a shape that moves between versions
/// should degrade to a missing column, not to a failed parse.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Migration {
    /// Absent for repeatable migrations, which have a description and no version.
    pub version: Option<String>,
    pub description: String,
    /// `Pending`, `Success`, `Failed`, and whatever else Flyway adds. Kept as
    /// **Flyway's own word** rather than mapped onto an enum of ours: an
    /// unknown state should reach the user as what it is, not as "other".
    pub state: String,
    pub category: Option<String>,
    /// `type` in the JSON — `SQL`, `JDBC`. Renamed because `type` is a keyword
    /// on one side and a meaningless field name on the other.
    pub kind: Option<String>,
    /// Absolute, and the reason this app never has to read `locations`.
    pub filepath: Option<String>,
    pub installed_on_utc: Option<String>,
    pub installed_by: Option<String>,
    pub execution_time_ms: Option<u64>,
}

impl Migration {
    pub fn is_pending(&self) -> bool {
        self.state.eq_ignore_ascii_case("pending")
    }
    pub fn is_failed(&self) -> bool {
        self.state.eq_ignore_ascii_case("failed")
    }
}

/// Flyway's own complaint, when it refuses to do something.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Complaint {
    pub code: Option<String>,
    /// **Verbatim.** Flyway explains itself well — "Detected failed migration
    /// to version 4 … run repair to fix the schema history" — and paraphrasing
    /// it would replace an instruction with a summary.
    pub message: String,
}

/// Pull `{ "error": { … } }` out of any operation's output.
///
/// Checked **before** the success shape, because a refused `migrate` returns an
/// object with nothing else in it — no `migrations`, no `success` — so code
/// that reads the success shape first sees an empty run rather than a refusal.
pub fn complaint(stdout: &str) -> Option<Complaint> {
    let v: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let e = v.get("error")?;
    Some(Complaint {
        code: e
            .get("errorCode")
            .and_then(|c| c.as_str())
            .map(str::to_string),
        message: e
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Flyway refused, without saying why.")
            .to_string(),
    })
}

/// The migrations `info` reported, in the order Flyway listed them.
pub fn migrations(stdout: &str) -> Result<Vec<Migration>, String> {
    let v: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|e| format!("Flyway's answer could not be read as JSON: {e}"))?;
    let list = v
        .get("migrations")
        .and_then(|m| m.as_array())
        .ok_or_else(|| "Flyway's answer carried no migrations list.".to_string())?;

    Ok(list
        .iter()
        .map(|m| {
            let text = |k: &str| m.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty());
            Migration {
                version: text("version").map(str::to_string),
                description: text("description")
                    .unwrap_or("(no description)")
                    .to_string(),
                state: text("state").unwrap_or("Unknown").to_string(),
                category: text("category").map(str::to_string),
                kind: text("type").map(str::to_string),
                filepath: text("filepath").map(str::to_string),
                installed_on_utc: text("installedOnUTC").map(str::to_string),
                installed_by: text("installedBy").map(str::to_string),
                execution_time_ms: m.get("executionTime").and_then(|x| x.as_u64()),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Real output**, taken from Flyway 13.5.0 against the fixture and
    /// trimmed to two migrations. Hand-written JSON here would only ever prove
    /// that the parser agrees with whoever wrote the test.
    const INFO: &str = r#"{
      "allSchemasEmpty": true,
      "database": "flyway_dev",
      "exception": null,
      "flywayVersion": "13.5.0",
      "licenseFailed": false,
      "migrations": [
        {
          "category": "Versioned",
          "description": "create widgets",
          "executionTime": 0,
          "filepath": "/home/x/dev/flyway/migrations/V1__create_widgets.sql",
          "installedBy": "",
          "installedOnUTC": "",
          "rawVersion": "1",
          "shouldExecuteExpression": null,
          "state": "Pending",
          "type": "SQL",
          "undoFilepath": "",
          "undoable": "No",
          "version": "1"
        },
        {
          "category": "Versioned",
          "description": "deliberately broken",
          "executionTime": 7,
          "filepath": "/home/x/dev/flyway/migrations/V4__deliberately_broken.sql",
          "installedBy": "root",
          "installedOnUTC": "2026-09-10T23:39:43Z",
          "rawVersion": "4",
          "shouldExecuteExpression": null,
          "state": "Failed",
          "type": "SQL",
          "undoFilepath": "",
          "undoable": "No",
          "version": "4"
        }
      ],
      "operation": "info",
      "schemaName": "",
      "schemaVersion": null,
      "timestamp": "2026-09-10T18:02:11.444948499-03:00"
    }"#;

    /// What a `migrate` returns when the history already holds a failure —
    /// **an object with nothing else in it**. Also real.
    const REFUSED: &str = r#"{"error": {"errorCode": "VALIDATE_ERROR", "message":
      "Validate failed: Migrations have failed validation\nDetected failed migration to version 4 (deliberately broken).\nPlease remove any half-completed changes then run repair to fix the schema history."}}"#;

    #[test]
    fn the_command_line_is_a_vector_in_the_order_flyway_takes() {
        let args = arguments("flyway.toml", "uat", "info", &[]);
        assert_eq!(
            args,
            [
                "-configFiles=flyway.toml",
                "-environment=uat",
                "-outputType=json",
                "info",
            ]
        );
    }

    /// Nothing is interpolated into a shell string, so this is an argument
    /// containing odd characters rather than a way to run something else.
    #[test]
    fn an_awkward_environment_name_stays_one_argument() {
        let args = arguments("a b.toml", "dev; rm -rf /", "info", &[]);
        assert_eq!(args[0], "-configFiles=a b.toml");
        assert_eq!(args[1], "-environment=dev; rm -rf /");
        assert_eq!(args.len(), 4, "still four arguments, not a command line");
    }

    #[test]
    fn extra_flags_come_before_the_operation() {
        let args = arguments("f.toml", "dev", "migrate", &["-outOfOrder=true".into()]);
        assert_eq!(args[3], "-outOfOrder=true");
        assert_eq!(
            args[4], "migrate",
            "the operation is what the flags apply to"
        );
    }

    #[test]
    fn info_is_read_with_flyways_own_field_names() {
        let m = migrations(INFO).expect("real info output");
        assert_eq!(m.len(), 2);

        assert_eq!(m[0].version.as_deref(), Some("1"));
        assert_eq!(m[0].description, "create widgets");
        assert!(m[0].is_pending());
        // `filepath`, not `script` — the documentation said otherwise, and this
        // is the field that makes "click to read it" possible at all.
        assert!(m[0]
            .filepath
            .as_deref()
            .unwrap()
            .ends_with("V1__create_widgets.sql"));
        // Empty strings are absent, not empty: a migration that has not run has
        // no installer and no date, and "" in a column is worse than nothing.
        assert_eq!(m[0].installed_on_utc, None);
        assert_eq!(m[0].installed_by, None);

        assert!(m[1].is_failed(), "the one repair exists for");
        // `installedOnUTC`, not `installedOn`.
        assert_eq!(
            m[1].installed_on_utc.as_deref(),
            Some("2026-09-10T23:39:43Z")
        );
        assert_eq!(m[1].kind.as_deref(), Some("SQL"));
        assert_eq!(m[1].execution_time_ms, Some(7));
    }

    /// The state is Flyway's word, kept. An unknown one must reach the user as
    /// itself rather than as "other".
    #[test]
    fn a_state_this_app_has_never_seen_survives() {
        let odd = INFO.replace("\"Pending\"", "\"Outdated\"");
        let m = migrations(&odd).unwrap();
        assert_eq!(m[0].state, "Outdated");
        assert!(!m[0].is_pending() && !m[0].is_failed());
    }

    #[test]
    fn a_refusal_is_found_before_anything_else_is_read() {
        let c = complaint(REFUSED).expect("a refused migrate");
        assert_eq!(c.code.as_deref(), Some("VALIDATE_ERROR"));
        assert!(c.message.contains("run repair"), "{}", c.message);
        // And the same output has no migrations list, which is exactly why the
        // refusal has to be looked for first.
        assert!(migrations(REFUSED).is_err());
    }

    #[test]
    fn ordinary_output_carries_no_complaint() {
        assert_eq!(complaint(INFO), None);
    }

    #[test]
    fn something_that_is_not_json_says_so_rather_than_panicking() {
        let e = migrations("Flyway is not installed").unwrap_err();
        assert!(e.contains("could not be read"), "{e}");
        assert_eq!(complaint("Flyway is not installed"), None);
    }

    /// The message somebody sees when they have not installed it, which is
    /// most people the first time.
    #[tokio::test]
    async fn a_missing_binary_says_where_to_fix_it() {
        let e = run(
            "definitely-not-a-real-program",
            std::path::Path::new("/tmp/flyway.toml"),
            "dev",
            "info",
            vec![],
        )
        .await
        .unwrap_err();
        assert!(e.contains("was not found"), "{e}");
        assert!(e.contains("Settings"), "it has to say where to fix it: {e}");
    }
}
