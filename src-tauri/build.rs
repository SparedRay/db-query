//! Build-time glue. Almost all of it is `tauri_build`; the exception is the
//! Windows application manifest, which this file takes over.
//!
//! **Why.** `tauri` links `muda` (menus) and `tray-icon` with the
//! `common-controls-v6` feature, which is on by default and which imports
//! entry points — `TaskDialogIndirect` among them — that only ComCtl32 **v6**
//! exports. A process gets v6 by declaring it in an application manifest;
//! without one it is handed the v5 stub, which exports fewer functions, and
//! the import cannot be resolved. That is a *load-time* failure: Windows
//! refuses to start the executable at all, before `main`, with
//!
//!     exit code: 0xc0000139, STATUS_ENTRYPOINT_NOT_FOUND
//!
//! `tauri-build` does embed that manifest — as a Windows *resource*, compiled
//! by `embed-resource`, which links resources with `cargo:rustc-link-arg-bins`.
//! Note the `-bins`: the app binary gets the manifest and **no test executable
//! ever does**. Any test that reaches Tauri's runtime therefore cannot be
//! loaded on Windows. Upstream: tauri-apps/tauri#13419, open since 2025-05.
//!
//! `tests/mcp_http.rs` is the first test in this project to build a Tauri app —
//! `tauri::test::mock_app()` — and it failed on Windows CI in exactly this way
//! while passing on Linux, which has no such thing as ComCtl32.
//!
//! **The fix.** Take the manifest off `tauri-build` and hand the same XML to
//! the linker through `cargo:rustc-link-arg`, which has no `-bins` in it and so
//! covers binaries, tests, examples and benchmarks alike. `windows-app-manifest.xml`
//! is a byte-for-byte copy of the one `tauri-build` embeds, so the shipped
//! binary is unchanged — this moves *where* the manifest comes from, not what
//! is in it.
//!
//! Deliberately not `#[cfg(windows)]`, which in a build script is the *host*:
//! the question is what is being built, so it is asked of the target.

use std::env;
use std::path::PathBuf;

fn main() {
    let mut attributes = tauri_build::Attributes::new();

    // MSVC only: `/MANIFEST:EMBED` is a `link.exe` flag, and the GNU toolchain
    // neither understands it nor needs it — its manifest arrives through
    // windres with the rest of the resources.
    if cargo_cfg("CARGO_CFG_TARGET_OS") == "windows" && cargo_cfg("CARGO_CFG_TARGET_ENV") == "msvc"
    {
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        embed_app_manifest();
    }

    tauri_build::try_build(attributes).expect("tauri-build failed");
}

fn cargo_cfg(key: &str) -> String {
    env::var(key).unwrap_or_default()
}

fn embed_app_manifest() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"))
        .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    // No `/WX`. It is in the upstream workaround, and it would turn every
    // linker warning in the tree — including ones from crates we do not own —
    // into a failed Windows build. The proof that this works is the Windows CI
    // step that could not run an MCP test before it.
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
