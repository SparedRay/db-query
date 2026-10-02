//! Running an external program without putting a console window on screen.
//!
//! # Why this module exists
//!
//! On Windows, a GUI application that spawns a console program gets a console
//! **window**: `cmd.exe` opens in front of the app, steals focus, and closes
//! when the program exits. Reported against Flyway, where clicking Repair
//! looked like it did nothing — the window covered the app, so the "Flyway is
//! repairing…" message and the button's own state were behind it.
//!
//! It is not only Flyway. Every external program this app runs is spawned the
//! same way, so Claude Code and Copilot CLI would flash a window **on every
//! question**. That is why this is one helper rather than a flag added where it
//! was noticed: the next spawn site should be born with it.
//!
//! Nothing is lost by hiding it. Every call site already captures stdout and
//! stderr — `output()` or a piped reader — so the window never held anything
//! the app did not have; it only showed it sooner, and over the top.
//!
//! # The flag
//!
//! `CREATE_NO_WINDOW` (0x0800_0000) tells Windows to run the child with no
//! console. It is a creation flag, so it has to be set before the spawn; there
//! is no undoing it afterwards.
//!
//! Elsewhere there is nothing to do — spawning a program on Linux or macOS
//! shows no window — so these are the same `Command` the callers used to build
//! themselves, and the Windows branch is the whole content of this file.

/// Windows' own constant, named here rather than depended on: pulling in
/// `windows-sys` for one `u32` would add a crate to every platform's build.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// An async `Command` that will not open a console window.
///
/// Use this instead of `tokio::process::Command::new`, everywhere.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    // `mut` is used by the Windows branch below and only there, so every other
    // platform would warn about it. Allowed narrowly rather than dropping the
    // `mut`, which would break the Windows build.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The blocking equivalent, for a program run inside `spawn_blocking`.
///
/// Flyway is run that way: it is a JVM that takes seconds, and `output()` on a
/// blocking thread keeps that off the async runtime's workers.
pub fn blocking_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    // `mut` is used by the Windows branch below and only there, so every other
    // platform would warn about it. Allowed narrowly rather than dropping the
    // `mut`, which would break the Windows build.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
