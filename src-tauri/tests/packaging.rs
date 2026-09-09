//! What the release depends on being true about the configuration.
//!
//! These are one-line facts that no other test would notice breaking, and each
//! one has already cost a release:
//!
//!   * `v0.2.0` shipped a build calling itself `0.1.0`, because the version was
//!     a literal in `tauri.conf.json` that tagging did not touch. Since
//!     `latest.json` takes its version from the build rather than from the tag,
//!     every install was told it was already current.
//!   * The updater reads a URL that only resolves for **published** releases,
//!     which is easy to change to something that looks equivalent and is not.

use std::path::Path;

fn config() -> serde_json::Value {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
            .expect("tauri.conf.json must be readable");
    serde_json::from_str(&text).expect("tauri.conf.json must be valid JSON")
}

/// The version must be a *reference*, not a number.
///
/// A literal here builds and passes every other test — and quietly breaks
/// updates for everyone, because CI's tag stamp writes `package.json` and this
/// file would stop reading it.
#[test]
fn the_app_version_is_read_from_package_json() {
    let cfg = config();
    let version = cfg["version"]
        .as_str()
        .expect("tauri.conf.json must set `version`");
    assert_eq!(
        version, "../package.json",
        "the app version must point at package.json, which is what CI stamps \
         from the release tag (scripts/set-version.mjs). A literal version here \
         means every release advertises whatever number was last committed."
    );

    // And the file it points at has to be there, relative to this manifest.
    let pkg = Path::new(env!("CARGO_MANIFEST_DIR")).join("../package.json");
    assert!(pkg.exists(), "{} must exist", pkg.display());
}

/// Without this, `tauri build` produces **no signatures**, so `tauri-action`
/// has nothing to put in `latest.json` and attaches none — and every release
/// looks complete while the updater has nothing to read.
///
/// That is what happened: `v0.3.5` was published with the `.deb` and the
/// `-setup.exe` and no `latest.json` at all. The signing key was configured,
/// the workflow asked for the manifest, and CI passed. Tauri v2 only creates
/// updater artifacts when this is set:
/// <https://v2.tauri.app/plugin/updater/>
#[test]
fn the_bundler_is_told_to_create_updater_artifacts() {
    let cfg = config();
    assert_eq!(
        cfg["bundle"]["createUpdaterArtifacts"].as_bool(),
        Some(true),
        "bundle.createUpdaterArtifacts must be true, or the release ships without \
         latest.json and no install can ever update itself. Nothing else fails when \
         this is missing — not the build, not CI, not the release job."
    );
}

/// The endpoint and the key are the other two halves. A release with a manifest
/// nobody can verify is no better than one with no manifest.
#[test]
fn the_updater_is_configured_end_to_end() {
    let cfg = config();
    let updater = &cfg["plugins"]["updater"];
    assert!(
        updater["pubkey"].as_str().is_some_and(|k| !k.is_empty()),
        "the updater needs the public key matching the private key CI signs with"
    );
    let endpoint = updater["endpoints"][0]
        .as_str()
        .expect("the updater needs an endpoint");
    assert!(
        endpoint.ends_with("/releases/latest/download/latest.json"),
        "the endpoint must be the release asset the workflow attaches, got {endpoint}"
    );
}

/// package.json is the source of truth, but a crate that disagrees with the app
/// it builds is a trap for whoever reads it next. `set-version` moves both.
#[test]
fn the_crate_version_agrees_with_the_app_version() {
    let pkg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../package.json"))
            .expect("package.json must be readable"),
    )
    .expect("package.json must be valid JSON");

    assert_eq!(
        pkg["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "package.json and Cargo.toml disagree about the version. \
         Run `mise run set-version <x.y.z>` rather than editing either by hand."
    );
}

/// The updater endpoint must be the *published*-release URL.
///
/// `releases/latest/download/...` resolves only for a published release; a
/// draft is not `latest`, and the check fails with a 404 that reaches the user
/// as an error rather than as "there is nothing new".
#[test]
fn the_updater_reads_published_releases() {
    let cfg = config();
    let endpoints = cfg["plugins"]["updater"]["endpoints"]
        .as_array()
        .expect("the updater must have endpoints");
    assert!(!endpoints.is_empty(), "at least one endpoint is required");

    let first = endpoints[0].as_str().unwrap_or_default();
    assert!(
        first.contains("/releases/latest/download/latest.json"),
        "unexpected updater endpoint: {first}"
    );
}

/// A build with no public key cannot be told to trust one later — those installs
/// are simply not updatable, and every one has to be replaced by hand.
#[test]
fn the_updater_has_a_public_key() {
    let cfg = config();
    let pubkey = cfg["plugins"]["updater"]["pubkey"]
        .as_str()
        .unwrap_or_default();
    assert!(!pubkey.trim().is_empty(), "the updater needs a pubkey");
}

/// The licence notices must be **in the bundle**.
///
/// Generating them and forgetting to ship them is the failure mode that looks
/// exactly like success: the file exists in the tree, CI is green, and every
/// installer goes out without it.
#[test]
fn the_licence_notices_are_bundled() {
    let cfg = config();
    let resources = cfg["bundle"]["resources"]
        .as_array()
        .expect("bundle.resources must list the licence file");
    assert!(
        resources
            .iter()
            .any(|r| r.as_str() == Some(db_query_lib::LICENSES_FILE)),
        "bundle.resources must contain {} — otherwise the notices are generated \
         and then left behind. Found: {resources:?}",
        db_query_lib::LICENSES_FILE
    );
}

/// And they must be **generated**, or the resource is a broken path.
///
/// `beforeBuildCommand` is the only hook that runs for every bundle on every
/// platform, which is why the generator lives there rather than in a release
/// workflow someone could forget to copy.
#[test]
fn packaging_generates_the_licence_notices() {
    let cfg = config();
    let before = cfg["build"]["beforeBuildCommand"]
        .as_str()
        .unwrap_or_default();
    assert!(
        before.contains("attribution"),
        "beforeBuildCommand must generate the licence notices, or a bundle can \
         be built without them. Found: {before:?}"
    );
}
