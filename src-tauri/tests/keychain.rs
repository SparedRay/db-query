//! Keychain round trip against the machine's real credential store.
//!
//! Ignored by default: it touches the user's keychain, and CI boxes often have
//! none. Run with:
//!
//!     cargo test --test keychain -- --ignored --nocapture
//!
//! **This is the Linux half of milestone C3. The Windows half (C3w) can only be
//! run on Windows** — cross-compilation proves the backend exists, not that it
//! behaves.

use db_query_lib::secrets::{self, Secret};

/// A per-run id so a failed run cannot poison the next one.
fn id() -> String {
    format!(
        "db-query-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
#[ignore]
fn store_load_delete_round_trip() {
    let id = id();
    assert!(!secrets::has_stored(&id), "id was not clean to begin with");

    secrets::store(&id, &Secret::new("hunter2")).expect("store failed");
    assert!(secrets::has_stored(&id));

    let got = secrets::load(&id).unwrap().expect("nothing stored");
    assert_eq!(got.expose(), "hunter2");

    secrets::delete(&id).expect("delete failed");
    assert!(
        !secrets::has_stored(&id),
        "the secret outlived its deletion — this is the orphaned-credential leak"
    );
    assert!(secrets::load(&id).unwrap().is_none());
}

#[test]
#[ignore]
fn overwriting_replaces_rather_than_appends() {
    let id = id();
    secrets::store(&id, &Secret::new("first")).unwrap();
    secrets::store(&id, &Secret::new("second")).unwrap();
    assert_eq!(secrets::load(&id).unwrap().unwrap().expose(), "second");
    secrets::delete(&id).unwrap();
}

#[test]
#[ignore]
fn loading_an_unknown_id_is_none_not_an_error() {
    // "Nothing stored" is a normal state, not a failure.
    assert!(secrets::load(&id()).unwrap().is_none());
}

#[test]
#[ignore]
fn deleting_something_that_is_not_there_succeeds() {
    // The goal is "not there", which it already is.
    secrets::delete(&id()).expect("deleting a missing entry should succeed");
}

#[test]
#[ignore]
fn secrets_with_awkward_bytes_survive() {
    let id = id();
    // Unicode, quotes, and a NUL-adjacent mix — passwords are not identifiers.
    let awkward = "p@ss «wörd» \"'\\ ünïcode → 🔐";
    secrets::store(&id, &Secret::new(awkward)).unwrap();
    assert_eq!(secrets::load(&id).unwrap().unwrap().expose(), awkward);
    secrets::delete(&id).unwrap();
}
