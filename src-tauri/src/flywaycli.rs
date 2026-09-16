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

// ------------------------------------------------------- finding the program

/// The extensions a bare program name may be wearing, in the order to try.
///
/// **Windows only, and the reason any of this exists.** `CreateProcessW`
/// appends `.exe` and nothing else when it searches the PATH, and Rust's
/// `Command` does the same — `resolve_exe` in `std` calls
/// `set_extension("exe")` and never looks at `PATHEXT`. The Flyway
/// distribution ships `flyway.cmd`, so `Command::new("flyway")` on Windows
/// reports *program not found* for a Flyway that is sitting on the PATH in
/// plain sight.
///
/// Handing the resolved `.cmd` back to `Command` is safe: `std` spots the
/// extension and runs it through `cmd.exe` with batch-specific quoting, which
/// is the fix for CVE-2024-24576 and is why this does not build a shell string
/// of its own.
///
/// `.cmd` first because that is the file Flyway ships; the other two cost a
/// `stat` each and cover a wrapper somebody wrote themselves.
#[cfg(windows)]
pub const EXTENSIONS: &[&str] = &["cmd", "bat", "exe"];
/// Empty everywhere else. `execvp` searches the PATH itself and there are no
/// extensions to guess, so the name is passed through untouched and Linux and
/// macOS keep the behaviour they already had.
#[cfg(not(windows))]
pub const EXTENSIONS: &[&str] = &[];

/// Where a program was looked for, and what turned up.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// What to hand to `Command::new`: the file found, or — when nothing was —
    /// the name as given, so the operating system's own error is what the user
    /// sees rather than a guess of ours.
    pub program: String,
    /// Every path tried, and whether a file was there. Empty where the
    /// operating system does the searching. This is the diagnostic.
    pub tried: Vec<(PathBuf, bool)>,
    /// The one that answered.
    pub found: Option<PathBuf>,
}

/// Every path worth trying for `program`, in order.
///
/// Pure, and takes its world as arguments — the PATH already split, and the
/// extensions — so the **Windows rules can be tested on a Linux machine**,
/// which is the only kind this is written on.
pub fn candidates(program: &str, dirs: &[PathBuf], exts: &[&str]) -> Vec<PathBuf> {
    // A name already wearing an extension is taken at its word: somebody who
    // typed `flyway.cmd` has said which file they mean.
    let spellings: Vec<String> = if exts.is_empty() || Path::new(program).extension().is_some() {
        vec![program.to_string()]
    } else {
        exts.iter()
            .map(|e| format!("{program}.{e}"))
            // The bare name last, for a Unix-style wrapper with no extension.
            .chain(std::iter::once(program.to_string()))
            .collect()
    };

    // A path is a path. Only a bare name is looked for along the PATH.
    if program.contains(['/', '\\']) {
        return spellings.iter().map(PathBuf::from).collect();
    }
    // Every spelling within one directory before moving to the next, which is
    // what a shell does and what somebody who put a wrapper early in their
    // PATH is expecting.
    dirs.iter()
        .flat_map(|d| spellings.iter().map(|s| d.join(s)))
        .collect()
}

/// Find `program`, on the platforms where the operating system will not.
pub fn resolve(program: &str) -> Resolution {
    if EXTENSIONS.is_empty() {
        return Resolution {
            program: program.to_string(),
            tried: Vec::new(),
            found: None,
        };
    }
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let tried: Vec<(PathBuf, bool)> = candidates(program, &dirs, EXTENSIONS)
        .into_iter()
        .map(|c| {
            let there = c.is_file();
            (c, there)
        })
        .collect();
    let found = tried
        .iter()
        .find(|(_, there)| *there)
        .map(|(p, _)| p.clone());
    Resolution {
        program: found
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| program.to_string()),
        tried,
        found,
    }
}

// ------------------------------------------------------------- the report

/// How many misses to print before giving up on the list. A Windows PATH with
/// forty entries produces a hundred and sixty candidates, and nobody is going
/// to read — or paste — that.
const MISS_LIMIT: usize = 40;

/// A live probe: go looking for Flyway, run it, and say what happened.
///
/// **A section of the diagnostics report, not a report of its own.** The
/// header and the log tail belong to `diagnostics` in `lib.rs`; this answers
/// the three questions that come up when Flyway will not start — which
/// command, where was it looked for, and what did the program itself say —
/// none of which a user can answer from "Flyway was not found".
///
/// It carries no secret: the project file, the JDBC URL and the connection are
/// not part of it. It does carry file paths, and on Windows effectively the
/// PATH, which is why the report says so at the top.
pub async fn probe(program: &str) -> String {
    let asked = if program.trim().is_empty() {
        DEFAULT_PROGRAM
    } else {
        program.trim()
    };
    let r = resolve(asked);

    let mut out = String::new();
    out.push_str(&format!(
        "setting    {}\n",
        if program.trim().is_empty() {
            format!("(empty \u{2014} using the default, {DEFAULT_PROGRAM:?})")
        } else {
            format!("{:?}", program.trim())
        }
    ));

    if r.tried.is_empty() {
        out.push_str("search     left to the operating system\n");
    } else if let Some(found) = &r.found {
        out.push_str(&format!(
            "search     {} candidates, Windows appends only .exe so .cmd is tried here\n",
            r.tried.len()
        ));
        out.push_str(&format!("found      {}\n", found.display()));
    } else {
        out.push_str(&format!(
            "search     {} candidates, none of them a file:\n",
            r.tried.len()
        ));
        for (c, _) in r.tried.iter().take(MISS_LIMIT) {
            out.push_str(&format!("             {}\n", c.display()));
        }
        if r.tried.len() > MISS_LIMIT {
            out.push_str(&format!(
                "             \u{2026} and {} more\n",
                r.tried.len() - MISS_LIMIT
            ));
        }
    }
    out.push_str(&format!("running    {:?} -v\n\n", r.program));

    match spawn(asked, vec!["-v".to_string()], None).await {
        Err(e) => out.push_str(&format!("FAILED     {e}\n")),
        Ok(f) => {
            out.push_str(&format!(
                "exit       {}\n",
                f.code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "killed by a signal".into())
            ));
            out.push_str(&section("stdout", &f.stdout));
            out.push_str(&section("stderr", &f.stderr));
        }
    }
    out
}

/// How much of a program's output to keep. `flyway -v` prints its whole plugin
/// table — forty lines of versions for databases nobody here uses — and the
/// line that matters is the second one. A report nobody will paste because it
/// is too long answers nothing.
const OUTPUT_LINES: usize = 12;

/// One labelled block of a program's output, or a note that it said nothing.
fn section(name: &str, text: &str) -> String {
    let body = text.trim_end();
    if body.is_empty() {
        return format!("\n--- {name} ---\n(nothing)\n");
    }
    let lines: Vec<&str> = body.lines().collect();
    let kept = lines
        .iter()
        .take(OUTPUT_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let rest = lines.len().saturating_sub(OUTPUT_LINES);
    if rest == 0 {
        format!("\n--- {name} ---\n{kept}\n")
    } else {
        format!("\n--- {name} ---\n{kept}\n\u{2026} and {rest} more lines\n")
    }
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
    spawn(program, args, Some(dir)).await
}

/// Start a program, wait for it, and turn the ways that can fail into
/// sentences. Shared by `run` and by `diagnose`, so the report a user sends us
/// is produced by the same code path it is a report about.
async fn spawn(program: &str, args: Vec<String>, dir: Option<PathBuf>) -> Result<Finished, String> {
    // What the user asked for, for the message; what was found, for the spawn.
    let shown = program.to_string();
    let r = resolve(program);
    match (&r.found, r.tried.len()) {
        (Some(found), n) => crate::logbook::info(
            "flyway",
            format!(
                "resolved {shown:?} to {} after {n} candidates",
                found.display()
            ),
        ),
        (None, 0) => crate::logbook::info("flyway", format!("running {shown:?} as given")),
        (None, n) => crate::logbook::warn(
            "flyway",
            format!("{shown:?} matched none of {n} candidates; trying it as given"),
        ),
    }
    let program = r.program;

    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(&program);
        cmd.args(&args)
            // Flyway asks nothing interactively, but a program that decided to
            // would otherwise hang a click forever with no way to answer it.
            .stdin(std::process::Stdio::null());
        if let Some(dir) = dir {
            cmd.current_dir(&dir);
        }
        let out = cmd.output().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "Flyway was not found at \"{shown}\". Install the Flyway command line, or set \
                 the path to it in Settings \u{2192} Integrations. Settings \u{2192} About \
                 \u{2192} Diagnostics prints every path that was tried."
            ),
            std::io::ErrorKind::PermissionDenied => {
                format!("\"{shown}\" cannot be run: permission denied.")
            }
            _ => format!("Could not run \"{shown}\": {e}"),
        });
        let out = match out {
            Ok(out) => out,
            Err(e) => {
                crate::logbook::error("flyway", &e);
                return Err(e);
            }
        };
        let finished = Finished {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        };
        // The arguments are not recorded: `-environment=uat` is harmless and
        // the habit of logging an argument vector is not. The operation is the
        // last one, and it is the part worth knowing.
        let op = args.last().map(String::as_str).unwrap_or("?");
        let code = finished
            .code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        crate::logbook::note(
            if finished.ok() {
                crate::logbook::Level::Info
            } else {
                crate::logbook::Level::Warn
            },
            "flyway",
            format!("{op} exited {code}"),
        );
        Ok(finished)
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
    /// Derived from `state`, and sent so the renderer never has to know what
    /// Flyway's words mean. Set by `migrations()`.
    pub group: Group,
}

/// What the next `migrate` would do with a migration.
///
/// **The axis the pane groups on**, and the one that matters before applying:
/// not "has this run" but "will this run". Flyway has a dozen states and most
/// of them — `Ignored`, `Superseded`, `Above Baseline`, `Missing` — differ in
/// why they will not run rather than in whether they will.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    /// Flyway will run it.
    Pending,
    /// It failed, and it blocks every other migration until it is repaired.
    Failed,
    /// Applied, skipped, superseded, baselined. `migrate` will not touch it.
    Done,
}

impl Migration {
    pub fn is_pending(&self) -> bool {
        self.state.eq_ignore_ascii_case("pending")
    }

    /// `Failed`, but also `Failed (Missing)` and `Failed (Future)`, which are
    /// the same problem wearing a qualifier.
    pub fn is_failed(&self) -> bool {
        self.state.to_ascii_lowercase().contains("failed")
    }

    /// Which group this belongs to.
    ///
    /// **`Done` is the default, including for a state this build has never
    /// seen.** Under-promising is the safe direction: claiming something will
    /// not run and being wrong shows up as an extra line in what Flyway
    /// reports it executed, which is visible. Claiming something *will* run
    /// and being wrong is a confirmation dialog that lied about what it was
    /// asking permission for.
    ///
    /// Note that `Ignored` lands in `Done` — correctly, because it is only
    /// ignored while out-of-order is off. Turn it on and Flyway reports the
    /// same migration as `Pending`, which is why the pane re-asks rather than
    /// reasoning about it here.
    pub fn group(&self) -> Group {
        if self.is_failed() {
            Group::Failed
        } else if self.is_pending() {
            Group::Pending
        } else {
            Group::Done
        }
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
                // Overwritten immediately below; `group()` needs the state,
                // which is not set until the struct exists.
                group: Group::Done,
            }
        })
        .map(|m| Migration {
            group: m.group(),
            ..m
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

    /// The grouping the pane hangs on, and the one the apply dialog counts.
    #[test]
    fn a_migration_is_grouped_by_what_apply_would_do_with_it() {
        let m = migrations(INFO).unwrap();
        assert_eq!(m[0].group(), Group::Pending, "Pending runs");
        assert_eq!(m[1].group(), Group::Failed, "Failed blocks everything");

        let state = |s: &str| Migration {
            state: s.to_string(),
            ..m[0].clone()
        };
        // Whatever the reason, none of these will be executed.
        for done in [
            "Success",
            "Ignored",
            "Superseded",
            "Above Baseline",
            "Baseline",
            "Missing",
            "Out of Order",
            "Undone",
        ] {
            assert_eq!(state(done).group(), Group::Done, "{done}");
        }
        // A qualifier does not stop a failure being one.
        assert_eq!(state("Failed (Missing)").group(), Group::Failed);
        assert_eq!(state("Failed (Future)").group(), Group::Failed);
        // And a state from a later Flyway is Done: under-promising is the only
        // safe direction for a dialog that asks permission to run things.
        assert_eq!(state("Something Flyway Adds In 2027").group(), Group::Done);
    }

    /// The group travels with the migration, so the renderer never has to
    /// learn Flyway's vocabulary to bucket a row.
    #[test]
    fn the_group_is_set_when_the_list_is_parsed() {
        let m = migrations(INFO).unwrap();
        assert_eq!(m[0].group, Group::Pending);
        assert_eq!(m[1].group, Group::Failed);
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

    /// The Windows extensions, spelled out, so a change to them is a change to
    /// a test rather than a surprise on somebody else's machine.
    const WIN: &[&str] = &["cmd", "bat", "exe"];

    fn dirs(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// The bug this whole section exists for. Flyway Desktop installs
    /// `flyway.cmd`; Windows appends `.exe` and only `.exe`; so the one
    /// spelling that matters is the one nobody would have tried.
    ///
    /// **Built with `join` rather than written out**, here and below. A
    /// candidate is a path, and the separator in it belongs to the platform —
    /// comparing `display()` against a hand-typed `a/flyway.cmd` tests this
    /// file's spelling rather than the rule, and fails on Windows for a reason
    /// that has nothing to do with Flyway. Which it did.
    #[test]
    fn a_bare_name_on_windows_is_tried_as_cmd_first() {
        let tools = PathBuf::from("C:\\tools");
        assert_eq!(
            candidates("flyway", std::slice::from_ref(&tools), WIN),
            vec![
                tools.join("flyway.cmd"),
                tools.join("flyway.bat"),
                tools.join("flyway.exe"),
                tools.join("flyway"),
            ]
        );
    }

    /// Every spelling in one directory before moving on, so a wrapper put early
    /// in the PATH wins over an installation later in it.
    #[test]
    fn directories_are_exhausted_one_at_a_time() {
        let c = candidates("flyway", &dirs(&["a", "b"]), WIN);
        assert_eq!(c.len(), 8);
        assert_eq!(c[0], PathBuf::from("a").join("flyway.cmd"));
        assert_eq!(c[4], PathBuf::from("b").join("flyway.cmd"), "b starts at 4");
    }

    /// Somebody who typed `flyway.cmd` has said which file they mean, and
    /// `flyway.cmd.exe` is not a thing.
    #[test]
    fn a_name_that_already_has_an_extension_is_taken_at_its_word() {
        let c = candidates("flyway.cmd", &dirs(&["C:\\tools"]), WIN);
        assert_eq!(c, dirs(&["C:\\tools/flyway.cmd"]));
    }

    /// A path is a path: the PATH is not searched, but the extension still is,
    /// because "C:\\Flyway\\flyway" is what a person types when pointing at a
    /// folder they installed.
    #[test]
    fn a_path_keeps_its_folder_and_gains_the_extensions() {
        let c = candidates(
            "C:\\Program Files\\Flyway\\flyway",
            &dirs(&["ignored"]),
            WIN,
        );
        let names: Vec<String> = c.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(names[0], "C:\\Program Files\\Flyway\\flyway.cmd");
        assert_eq!(names.len(), 4);
        assert!(
            names.iter().all(|n| !n.contains("ignored")),
            "the PATH has no say once a path is given: {names:?}"
        );
    }

    /// A folder with a dot in it must not be read as an extension on the
    /// program — `flyway-13.5.0/flyway` is exactly how the distribution
    /// unpacks, and reading `.5.0/flyway` as an extension would try nothing
    /// but the bare name.
    #[test]
    fn a_dot_in_a_folder_is_not_an_extension_on_the_program() {
        let c = candidates("/opt/flyway-13.5.0/flyway", &[], WIN);
        let names: Vec<String> = c.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(names[0], "/opt/flyway-13.5.0/flyway.cmd");
        assert_eq!(names.len(), 4);
    }

    /// With no extensions to guess — Linux, macOS — nothing is invented, and
    /// the platforms that already worked keep working.
    #[test]
    fn nowhere_else_guesses_anything() {
        assert_eq!(
            candidates("flyway", &dirs(&["/usr/bin"]), &[]),
            dirs(&["/usr/bin/flyway"])
        );
        // And `resolve` hands the name straight back rather than searching.
        let r = resolve("flyway");
        if EXTENSIONS.is_empty() {
            assert_eq!(r.program, "flyway");
            assert!(r.tried.is_empty(), "the OS does the looking here");
        }
    }

    /// The probe has to survive the case it is most often asked about.
    ///
    /// The build and the platform are asserted where they are now written —
    /// `diagnostics` in `lib.rs`, which owns the header this section sits
    /// under.
    #[tokio::test]
    async fn the_probe_describes_a_flyway_that_is_not_there() {
        let text = probe("definitely-not-a-real-program").await;
        assert!(text.contains("definitely-not-a-real-program"), "{text}");
        assert!(text.contains("FAILED"), "{text}");
        // Nothing about the connection or the project belongs in something
        // written to be pasted into a chat window.
        assert!(!text.contains("jdbc:"), "{text}");
    }

    /// An empty setting is the default, and the report says which it used
    /// rather than leaving a blank where the command should be.
    #[tokio::test]
    async fn an_empty_setting_reports_the_default_it_fell_back_to() {
        let text = probe("   ").await;
        assert!(text.contains("using the default"), "{text}");
        assert!(text.contains("\"flyway\""), "{text}");
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
