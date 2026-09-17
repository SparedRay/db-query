//! Self-update, and the two honest refusals around it.
//!
//! Two rules shape everything here:
//!
//!   * **The user decides.** Checking is automatic; *installing* never is. The
//!     frontend asks through `dialog.ts` and calls `update_install` only on a
//!     yes. This is the same rule that governs running SQL.
//!   * **Never offer what cannot be delivered.** Tauri's updater replaces an
//!     AppImage or a Windows installer in place; it cannot update a `.deb`,
//!     which is what we ship on Linux (see Stage 5 §4.4 for why). So on Linux
//!     this reports `Unsupported` *before* anyone is promised anything, rather
//!     than failing after a download.

use serde::Serialize;

/// What a check found. Tagged so the frontend matches on one field.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UpdateStatus {
    /// This build cannot update itself, and why.
    Unsupported {
        reason: String,
    },
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        version: String,
        notes: Option<String>,
        date: Option<String>,
    },
}

/// Why a `.deb` — or any non-Windows build — is not updatable.
///
/// Stated in full because it is user-facing: an app that silently never
/// updates is worse than one that says it does not.
pub const UNSUPPORTED_REASON: &str =
    "This build updates through its installer, which only the Windows package has. \
     On Linux, install the newer .deb from the releases page.";

/// Is self-update possible for the way *this* binary was packaged?
pub fn supported() -> bool {
    cfg!(target_os = "windows")
}

/// What a completed check writes to the logbook.
///
/// **The lesson of 2026-09-17**: a release went out, the update never
/// appeared, and nothing anywhere recorded why. The boot check is silent on
/// purpose — it must not interrupt — but silent to the *user* and silent in
/// the *log* are different promises, and only the first was intended.
///
/// The wording lives here rather than inline in the command for two reasons:
/// most of it is behind `#[cfg(target_os = "windows")]`, which no machine but
/// CI compiles; and the whole value of these lines is that the outcomes are
/// *distinguishable*, which is a property of the words that nothing else was
/// checking.
pub fn checked(current: &str, offered: Option<&str>) -> String {
    match offered {
        Some(v) => format!("running {current}; the endpoint offers {v}"),
        None => format!("running {current}; the endpoint has nothing newer"),
    }
}

/// The same line for a build that was never going to update itself.
///
/// Separate from "nothing newer", because they call for opposite replies:
/// one means wait, the other means go and download a `.deb`.
pub fn cannot_update(current: &str) -> String {
    format!("running {current}; this build does not self-update")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_builds_say_so_rather_than_failing_later() {
        // The point is that the refusal is knowable without a network call.
        assert_eq!(supported(), cfg!(target_os = "windows"));
    }

    #[test]
    fn the_refusal_tells_the_user_what_to_do_instead() {
        // A reason that does not name the alternative is an error message, not
        // an explanation.
        assert!(UNSUPPORTED_REASON.contains(".deb"));
        assert!(UNSUPPORTED_REASON.contains("releases"));
    }

    /// The four outcomes have to be told apart in a log somebody is reading
    /// because an update did not appear. Before this they were one absence.
    #[test]
    fn a_check_says_which_of_the_outcomes_it_was() {
        let offers = checked("0.4.4", Some("0.4.5"));
        let nothing = checked("0.4.5", None);
        let never = cannot_update("0.4.5");

        // Each names the version actually running, which is the first thing
        // anybody asks and the thing screenshots never show.
        assert!(offers.contains("0.4.4"), "{offers}");
        assert!(nothing.contains("0.4.5"), "{nothing}");
        assert!(never.contains("0.4.5"), "{never}");

        // And the one that found something says what it found.
        assert!(offers.contains("0.4.5"), "{offers}");

        // Distinguishable from each other, not merely different in tone. A
        // reader scanning for "why is there no update" must land on one.
        assert_ne!(offers, nothing);
        assert_ne!(nothing, never);
        assert_ne!(offers, never);
    }

    #[test]
    fn status_serialises_with_a_tag_the_ui_can_match_on() {
        let j = serde_json::to_value(UpdateStatus::Available {
            current: "0.1.0".into(),
            version: "0.1.1".into(),
            notes: Some("Fixes".into()),
            date: None,
        })
        .unwrap();
        assert_eq!(j["type"], "available");
        assert_eq!(j["version"], "0.1.1");
        assert_eq!(j["current"], "0.1.0");
    }

    #[test]
    fn up_to_date_still_reports_the_running_version() {
        let j = serde_json::to_value(UpdateStatus::UpToDate {
            current: "0.1.1".into(),
        })
        .unwrap();
        assert_eq!(j["type"], "upToDate");
        assert_eq!(j["current"], "0.1.1");
    }
}
