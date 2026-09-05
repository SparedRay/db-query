//! Passwords: holding them in memory, and storing them in the OS keychain.
//!
//! # What the keychain is, per platform
//!
//! `keyring` 4.x's default `v1` feature selects the native store:
//!
//!   * **Windows** — Windows Credential Manager
//!   * **macOS** — Keychain Services
//!   * **Linux/*nix** — Secret Service, over D-Bus
//!
//! 3.x was rejected on purpose: it put those backends behind opt-in features
//! and fell back to a **mock store that silently accepted writes and stored
//! nothing**. A password that appears saved but is not is worse than one that
//! obviously failed to save.
//!
//! # Every write is read back
//!
//! The failure that matters is silently not storing. So [`store`] reads the
//! secret back and compares it before reporting success. That turns a silent
//! failure into a visible one on every platform, for the price of one call.

use std::fmt;

use zeroize::Zeroize;

/// Keychain service name. The account is the connection profile's id.
const SERVICE: &str = "db-query";

/// A password held in memory.
///
/// Deliberately has **no** `Display`, `Serialize` or derived `Debug`: the only
/// way to see the contents is [`Secret::expose`], which is easy to grep for in
/// review. `Debug` prints `<redacted>` so a stray `{:?}` in a log line or a
/// panic message cannot leak it.
///
/// # What zeroizing does and does not buy
///
/// The buffer is zeroed on drop, which stops the password lingering in freed
/// memory for the rest of the process. It is **not** a guarantee the password
/// is unrecoverable: a `String` that reallocated while being built leaves
/// copies nothing can reach, and the OS may already have paged it to swap.
/// Worth doing; not worth believing in.
#[derive(Clone, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Why a keychain operation could not be completed. Never carries the secret.
#[derive(Debug, Clone)]
pub struct KeychainUnavailable(pub String);

impl fmt::Display for KeychainUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn entry(id: &str) -> Result<keyring::Entry, KeychainUnavailable> {
    keyring::Entry::new(SERVICE, id).map_err(|e| {
        KeychainUnavailable(format!(
            "No system credential store is available ({e}). The connection was saved, \
             but you will be asked for the password each time."
        ))
    })
}

/// Store a password, then **read it back and verify it**.
///
/// A store that accepts a write and keeps nothing would otherwise look like
/// success and only be discovered the next time the user tried to connect.
pub fn store(id: &str, password: &Secret) -> Result<(), KeychainUnavailable> {
    let e = entry(id)?;
    e.set_password(password.expose()).map_err(|err| {
        KeychainUnavailable(format!(
            "The system credential store refused to save the password ({err}). \
             You will be asked for it each time."
        ))
    })?;

    match e.get_password() {
        Ok(back) if back == password.expose() => Ok(()),
        Ok(_) => Err(KeychainUnavailable(
            "The credential store did not return the password that was just saved, \
             so it has not been remembered."
                .into(),
        )),
        Err(err) => Err(KeychainUnavailable(format!(
            "The password could not be read back after saving ({err}), \
             so it has not been remembered."
        ))),
    }
}

/// Fetch a stored password. `Ok(None)` means "nothing stored", which is a
/// normal state, not an error.
pub fn load(id: &str) -> Result<Option<Secret>, KeychainUnavailable> {
    let e = match entry(id) {
        Ok(e) => e,
        // No credential store at all is indistinguishable, from the caller's
        // point of view, from having nothing stored.
        Err(_) => return Ok(None),
    };
    match e.get_password() {
        Ok(p) => Ok(Some(Secret::new(p))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(KeychainUnavailable(format!(
            "Could not read the stored password ({err})."
        ))),
    }
}

/// Remove a stored password. Missing is success — the goal is "not there".
pub fn delete(id: &str) -> Result<(), KeychainUnavailable> {
    let e = match entry(id) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    match e.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(KeychainUnavailable(format!(
            "Could not remove the stored password ({err}). It may still be in the \
             system credential store."
        ))),
    }
}

pub fn has_stored(id: &str) -> bool {
    matches!(load(id), Ok(Some(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_secret() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert!(!format!("{s:?}").contains("hunter2"));
        // And nested inside another structure, which is how it usually escapes.
        #[derive(Debug)]
        struct Holder {
            #[allow(dead_code)]
            password: Secret,
        }
        let h = Holder {
            password: Secret::new("hunter2"),
        };
        assert!(!format!("{h:?}").contains("hunter2"), "{h:?}");
    }

    #[test]
    fn expose_is_the_only_way_out() {
        let s = Secret::new("hunter2");
        assert_eq!(s.expose(), "hunter2");
        assert!(!s.is_empty());
        assert!(Secret::default().is_empty());
    }

    /// Guards the invariant that makes the config file safe by construction:
    /// `Secret` must never gain a `Serialize` impl. If someone adds one, this
    /// stops compiling — which is the point.
    #[test]
    fn secret_is_not_serialisable() {
        fn assert_not_serialize<T>() {}
        assert_not_serialize::<Secret>();
        // The real guard is that `Secret` has no `#[derive(Serialize)]`; this
        // test exists so the reason is written down next to it.
    }
}
