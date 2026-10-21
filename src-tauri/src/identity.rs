//! Device identity: one long-term X25519 static keypair. The 32-byte public key
//! IS the Device ID. Generated on first run via `snow`'s resolver, persisted as a
//! single 64-byte `public||private` blob. The private key never crosses the IPC
//! boundary.
//!
//! WHERE the blob lives depends on the platform:
//! - Windows / Linux: the OS credential store (Credential Manager / Secret
//!   Service), which never questions an app about its own entry.
//! - macOS: `identity.key` in LanBeam's data folder, readable by this user only
//!   (0600). The keychain releases an item only to the exact build that stored
//!   it — judged by code signature, and without a paid Developer ID every
//!   LanBeam build signs differently — so it demanded the login password at the
//!   first launch after every update. Builds up to 0.1.1 kept the key in the
//!   keychain; the first launch of a newer one copies it into the file
//!   ([`Identity::load_or_create_file`]), keeping the same Device ID.

use std::io::Write;
use std::path::Path;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use keyring::Entry;
use snow::Builder;
use zeroize::{Zeroize, Zeroizing};

use crate::consts::NOISE_PARAMS;
use crate::error::{LanBeamError, Result};

const KR_SERVICE: &str = "LanBeam";
const KR_ACCOUNT: &str = "identity-x25519-v1";

/// Keychain account name for a given test-instance suffix — the single source
/// of truth shared by [`Identity::load_or_create`] and [`delete_persisted`],
/// so a reset always deletes exactly the entry the next launch would read
/// (and a test instance's reset can never touch the primary's identity).
fn account_for(suffix: Option<&str>) -> String {
    match suffix {
        Some(s) => format!("{KR_ACCOUNT}-{s}"),
        None => KR_ACCOUNT.to_string(),
    }
}

/// macOS keeps the identity in a file rather than the keychain (module docs).
/// A constant rather than `#[cfg]` blocks so both stores compile — and their
/// tests run — on every platform; only the choice is per-platform.
const FILE_STORE: bool = cfg!(target_os = "macos");

/// File name for a given test-instance suffix, in the app's data folder — the
/// file-store twin of [`account_for`], with the same per-instance isolation.
pub fn file_for(suffix: Option<&str>) -> String {
    match suffix {
        Some(s) => format!("identity-{s}.key"),
        None => "identity.key".to_string(),
    }
}

pub struct Identity {
    pub public: [u8; 32],
    private: Zeroizing<[u8; 32]>,
}

impl Identity {
    /// Load the persisted identity, or generate + store one on first run.
    /// `account_suffix` gives a distinct identity per test instance (so two
    /// instances on one machine don't share a Device ID); `data_dir` is where the
    /// macOS file store keeps it. Only a MISSING identity is a first run: a store
    /// that can't be read, or holds a malformed blob, is an error — never a
    /// reason to generate.
    pub fn load_or_create(account_suffix: Option<&str>, data_dir: &Path) -> Result<Identity> {
        let entry = Entry::new(KR_SERVICE, &account_for(account_suffix))?;
        if FILE_STORE {
            Self::load_or_create_file(&data_dir.join(file_for(account_suffix)), &entry)
        } else {
            Self::load_or_create_in(&entry)
        }
    }

    /// The file store, with `legacy` — the keychain entry builds up to 0.1.1
    /// used — read only while there is no file yet.
    ///
    /// No file means a first run OR an older build's keychain identity. Taking
    /// the keychain's (same Device ID — no peer sees a fingerprint change) costs
    /// one last access prompt. The keychain copy is then LEFT where it is: this
    /// build never reads it again once the file exists, but an older build run
    /// later still finds the same key there instead of minting a new one — a
    /// device that flips between two IDs depending on which version starts is
    /// worse than a duplicate. A reset deletes both. An unreadable keychain
    /// fails exactly as in [`Identity::load_or_create_in`]: never a reason to
    /// mint a replacement.
    fn load_or_create_file(path: &Path, legacy: &Entry) -> Result<Identity> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let b = Zeroizing::new(bytes); // wiped on drop
                Self::from_blob(&b).ok_or_else(|| {
                    // Corruption, not a first run — same reasoning as a bad
                    // keychain blob: refuse, and leave the bytes in place.
                    LanBeamError::Crypto(format!(
                        "bad identity file {}: {} bytes, expected 64",
                        path.display(),
                        b.len()
                    ))
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = match legacy.get_secret() {
                    Ok(blob) => {
                        let b = Zeroizing::new(blob);
                        Self::from_blob(&b).ok_or_else(|| {
                            LanBeamError::Crypto(format!(
                                "bad identity blob in the keychain: {} bytes, expected 64",
                                b.len()
                            ))
                        })?
                    }
                    Err(keyring::Error::NoEntry) => generate_identity()?,
                    Err(e) => return Err(e.into()),
                };
                write_private_file(path, &id)?;
                Ok(id)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `public||private` back into an identity; `None` unless exactly 64 bytes.
    fn from_blob(blob: &[u8]) -> Option<Identity> {
        if blob.len() != 64 {
            return None;
        }
        Some(Identity {
            public: blob[..32].try_into().ok()?,
            private: Zeroizing::new(blob[32..].try_into().ok()?),
        })
    }

    /// [`Identity::load_or_create`] against a given entry. Split out so the tests
    /// can hand it a `keyring::mock` entry with an injected store failure — the
    /// real stores offer no way to deny access on demand.
    fn load_or_create_in(entry: &Entry) -> Result<Identity> {
        match entry.get_secret() {
            Ok(blob) if blob.len() == 64 => {
                let b = Zeroizing::new(blob); // wiped on drop
                let public: [u8; 32] = b[..32]
                    .try_into()
                    .map_err(|_| LanBeamError::Crypto("bad identity blob".into()))?;
                let private: [u8; 32] = b[32..]
                    .try_into()
                    .map_err(|_| LanBeamError::Crypto("bad identity blob".into()))?;
                Ok(Identity {
                    public,
                    private: Zeroizing::new(private),
                })
            }
            // First run — the ONLY case that may mint an identity: the store
            // answered, and it holds none.
            Err(keyring::Error::NoEntry) => {
                let id = generate_identity()?;
                let mut blob = [0u8; 64];
                blob[..32].copy_from_slice(&id.public);
                blob[32..].copy_from_slice(&*id.private);
                entry.set_secret(&blob)?;
                // [FIX-7] wipe the staging buffer IN PLACE. `Zeroizing::new(blob)` would
                // copy the `Copy` array and wipe only the copy, leaving the real bytes.
                blob.zeroize();
                Ok(id)
            }
            // A stored blob of the wrong size is corruption, not a first run:
            // generating over it would silently give this device a new ID and
            // destroy the bytes someone could inspect. Refuse instead.
            Ok(blob) => {
                let b = Zeroizing::new(blob); // may still hold key bytes — wiped on drop
                Err(LanBeamError::Crypto(format!(
                    "bad identity blob: {} bytes, expected 64",
                    b.len()
                )))
            }
            // Any other error — a declined keychain prompt, a locked Secret Service
            // collection, a transient platform failure — says nothing about whether
            // an identity EXISTS. Generating one here would overwrite the real key
            // as soon as the store let the write through: a new Device ID, and
            // "fingerprint changed" on every peer that pinned the old one. Fail
            // startup instead; once the store is reachable, the next launch finds
            // the key.
            Err(e) => Err(e.into()),
        }
    }

    /// base64url (no pad) of the 32-byte public key — the canonical Device ID / pin target.
    pub fn device_id(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.public)
    }

    /// Raw private key, fed to `snow::Builder::local_private_key` at handshake time (M2).
    #[allow(dead_code)] // used from M2 (Noise handshake)
    pub fn private_bytes(&self) -> &[u8; 32] {
        &self.private
    }
}

/// Delete the persisted identity (M5.7 reset). A missing one counts as
/// success: the goal state — "no stored identity, the next launch generates a
/// fresh one" — is already true, and a retried reset must not fail because its
/// own first attempt half-succeeded.
///
/// The keychain entry goes FIRST, on macOS too: an entry an older build left
/// behind would otherwise be migrated straight back on the next launch, undoing
/// the reset — and if it can't be deleted, nothing has been deleted yet.
pub fn delete_persisted(account_suffix: Option<&str>, data_dir: &Path) -> Result<()> {
    let entry = Entry::new(KR_SERVICE, &account_for(account_suffix))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(e.into()),
    }
    if FILE_STORE {
        match std::fs::remove_file(data_dir.join(file_for(account_suffix))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Write `id` as `public||private` to `path`: readable by this user only (0600
/// on Unix), and atomically — a crash mid-write leaves the old file or none,
/// never half a key. Written to a sibling temp file, flushed, renamed over.
fn write_private_file(path: &Path, id: &Identity) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("key.tmp");
    let _ = std::fs::remove_file(&tmp); // a crashed earlier attempt
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut blob = [0u8; 64];
    blob[..32].copy_from_slice(&id.public);
    blob[32..].copy_from_slice(&*id.private);
    let written = opts.open(&tmp).and_then(|mut f| {
        f.write_all(&blob)?;
        f.sync_all()
    });
    blob.zeroize(); // in place — see [FIX-7]
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Generate a fresh keypair WITHOUT touching the keychain. `pub` (but doc-
/// hidden, like the whole module) so tests can construct throwaway identities
/// (e.g. a `TransportCtx` for `handle_incoming`) instead of writing entries
/// into the user's real credential store.
#[doc(hidden)]
pub fn generate_identity() -> Result<Identity> {
    let params = NOISE_PARAMS
        .parse()
        .map_err(|_| LanBeamError::Crypto("invalid noise params".into()))?;
    let kp = Builder::new(params).generate_keypair()?;
    let public: [u8; 32] = kp
        .public
        .as_slice()
        .try_into()
        .map_err(|_| LanBeamError::Crypto("unexpected public key length".into()))?;
    let private: [u8; 32] = kp
        .private
        .as_slice()
        .try_into()
        .map_err(|_| LanBeamError::Crypto("unexpected private key length".into()))?;
    Ok(Identity {
        public,
        private: Zeroizing::new(private),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring::mock::MockCredential;

    #[test]
    fn generated_keys_have_correct_shape() {
        let id = generate_identity().expect("generate");
        assert_eq!(id.public.len(), 32);
        assert_eq!(id.private_bytes().len(), 32);
        assert_eq!(id.device_id().len(), 43); // base64url no-pad of 32 bytes
    }

    /// A fresh, empty folder for one test's identity file, removed on drop.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("lanbeam-identity-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("temp dir");
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // AC-K3: a second load returns the SAME identity — through the platform's
    // real store (the OS credential store; the file store on macOS), but under a
    // throwaway account and folder, never the app's own: a test-created entry in
    // the real keychain is one that every later build must ask permission to
    // read. That changes nothing this catches: a store that doesn't persist
    // (keyring's silent mock fallback) still hands the second load a fresh key.
    #[test]
    fn identity_is_stable_across_loads() {
        let sfx = format!("stable-{}", std::process::id());
        let dir = TempDir::new("stable");
        let a = Identity::load_or_create(Some(&sfx), &dir.0).expect("first load");
        let b = Identity::load_or_create(Some(&sfx), &dir.0).expect("second load");
        // before the asserts, so a failing run leaves nothing behind either
        delete_persisted(Some(&sfx), &dir.0).expect("cleanup");
        assert_eq!(a.public, b.public);
        assert_eq!(a.device_id(), b.device_id());
    }

    /// Suffix → account mapping: a reset targets exactly the entry the next
    /// launch would read, and an instance suffix maps to a DIFFERENT account
    /// than the primary — a test instance's reset must never delete the
    /// user's real identity.
    #[test]
    fn account_for_isolates_instances() {
        assert_eq!(account_for(None), KR_ACCOUNT);
        assert_eq!(account_for(Some("b")), format!("{KR_ACCOUNT}-b"));
        assert_ne!(account_for(Some("b")), account_for(None));
    }

    /// M5.7 reset precondition: deleting the persisted identity really forgets
    /// it (the next load generates a DIFFERENT keypair), and deleting a missing
    /// entry succeeds (idempotent reset). Scoped to a throwaway suffix so the
    /// user's real identity is never touched.
    #[test]
    fn delete_persisted_forgets_identity_and_is_idempotent() {
        let sfx = format!("del-{}", std::process::id());
        let dir = TempDir::new("del");
        let a = Identity::load_or_create(Some(&sfx), &dir.0).expect("create");
        delete_persisted(Some(&sfx), &dir.0).expect("delete existing entry");
        delete_persisted(Some(&sfx), &dir.0).expect("deleting a missing entry is success");
        let b = Identity::load_or_create(Some(&sfx), &dir.0).expect("regenerate");
        assert_ne!(
            a.device_id(),
            b.device_id(),
            "reset must yield a fresh identity"
        );
        // leave nothing behind in the user's credential store
        delete_persisted(Some(&sfx), &dir.0).expect("cleanup");
    }

    #[test]
    fn file_for_isolates_instances() {
        assert_eq!(file_for(None), "identity.key");
        assert_eq!(file_for(Some("b")), "identity-b.key");
        assert_ne!(file_for(Some("b")), file_for(None));
    }

    /// File store, first run: mints, writes a 64-byte file only this user can
    /// read, and never touches the keychain.
    #[test]
    fn file_store_first_run_writes_a_private_file() {
        let dir = TempDir::new("first");
        let path = dir.0.join(file_for(None));
        let legacy = mock_entry();
        let a = Identity::load_or_create_file(&path, &legacy).expect("first run creates");
        let bytes = std::fs::read(&path).expect("file written");
        assert_eq!(bytes[..32], a.public);
        assert_eq!(bytes[32..], a.private_bytes()[..]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the key file must be private to this user"
            );
        }
        assert!(
            !path.with_extension("key.tmp").exists(),
            "no temp file left behind"
        );
        assert!(
            matches!(legacy.get_secret(), Err(keyring::Error::NoEntry)),
            "nothing may be written to the keychain"
        );
        let b = Identity::load_or_create_file(&path, &legacy).expect("reload");
        assert_eq!(a.public, b.public);
        assert_eq!(a.private_bytes(), b.private_bytes());
    }

    /// Upgrading from a keychain build: the SAME identity is copied into the
    /// file — the Device ID peers pinned survives — and the keychain copy stays,
    /// so an older build run later still finds that key, not a new one.
    #[test]
    fn file_store_migrates_the_keychain_identity() {
        let dir = TempDir::new("migrate");
        let path = dir.0.join(file_for(None));
        let legacy = mock_entry();
        let old = Identity::load_or_create_in(&legacy).expect("an older build's identity");
        let moved = Identity::load_or_create_file(&path, &legacy).expect("migrates");
        assert_eq!(
            moved.public, old.public,
            "the Device ID must survive the move"
        );
        assert_eq!(moved.private_bytes(), old.private_bytes());
        assert!(path.exists());
        let kept = legacy.get_secret().expect("the keychain copy is kept");
        assert_eq!(kept[..32], old.public, "for an older build to find");
    }

    /// The keychain can't be read during the move (a declined prompt): still an
    /// error, no file minted, and the keychain identity untouched — the next
    /// launch can simply try again.
    #[test]
    fn file_store_unreadable_keychain_fails_without_minting() {
        let dir = TempDir::new("denied");
        let path = dir.0.join(file_for(None));
        let legacy = mock_entry();
        let old = Identity::load_or_create_in(&legacy).expect("an older build's identity");
        let mock: &MockCredential = legacy.get_credential().downcast_ref().expect("mock");
        mock.set_error(keyring::Error::NoStorageAccess(
            "keychain access denied".into(),
        ));
        assert!(matches!(
            Identity::load_or_create_file(&path, &legacy),
            Err(LanBeamError::Keyring(_))
        ));
        assert!(
            !path.exists(),
            "no identity may be minted while the real one is unreadable"
        );
        let blob = legacy
            .get_secret()
            .expect("the keychain entry is still there");
        assert_eq!(blob[..32], old.public);
    }

    /// A damaged file is corruption, not a first run: fail, and leave it be.
    #[test]
    fn file_store_bad_file_fails_without_overwriting() {
        let dir = TempDir::new("bad");
        let path = dir.0.join(file_for(None));
        std::fs::write(&path, [7u8; 32]).expect("seed");
        assert!(matches!(
            Identity::load_or_create_file(&path, &mock_entry()),
            Err(LanBeamError::Crypto(_))
        ));
        assert_eq!(std::fs::read(&path).expect("still there"), [7u8; 32]);
    }

    /// A `keyring::mock` entry, handed straight to `load_or_create_in` rather
    /// than installed with `set_default_credential_builder` — that is
    /// process-global, and would swap the real store out from under the
    /// keychain tests above.
    fn mock_entry() -> Entry {
        Entry::new_with_credential(Box::new(MockCredential::default()))
    }

    /// First run generates, and stores exactly what it returned as
    /// `public||private` — pinning the layout the load arm reads back.
    #[test]
    fn missing_entry_creates_and_stores_identity() {
        let entry = mock_entry();
        let id = Identity::load_or_create_in(&entry).expect("first run creates");
        let blob = entry.get_secret().expect("the new identity was stored");
        assert_eq!(blob[..32], id.public);
        assert_eq!(blob[32..], id.private_bytes()[..]);
    }

    #[test]
    fn existing_entry_loads_the_same_identity() {
        let entry = mock_entry();
        let a = Identity::load_or_create_in(&entry).expect("create");
        let b = Identity::load_or_create_in(&entry).expect("load");
        assert_eq!(a.public, b.public);
        assert_eq!(a.private_bytes(), b.private_bytes());
    }

    /// The regression: an unreadable store must fail the load, never mint a
    /// replacement. The mock spends its injected error on the read, so a write
    /// attempted after it WOULD land — exactly how a key gets silently rotated.
    #[test]
    fn store_errors_fail_without_replacing_the_identity() {
        let entry = mock_entry();
        let original = Identity::load_or_create_in(&entry).expect("create");
        let mock: &MockCredential = entry.get_credential().downcast_ref().expect("mock");
        for err in [
            keyring::Error::NoStorageAccess("keychain access denied".into()),
            keyring::Error::PlatformFailure("transient".into()),
        ] {
            mock.set_error(err);
            match Identity::load_or_create_in(&entry) {
                Err(LanBeamError::Keyring(_)) => {}
                Err(e) => panic!("expected a keyring error, got {e:?}"),
                Ok(_) => panic!("a store error must not yield an identity"),
            }
            let blob = entry.get_secret().expect("the entry is still there");
            assert_eq!(blob[..32], original.public, "the identity was replaced");
        }
    }

    /// Corruption is not a first run: fail, and leave the bytes in place.
    #[test]
    fn malformed_blob_fails_without_overwriting() {
        let entry = mock_entry();
        let corrupt = [7u8; 32];
        entry.set_secret(&corrupt).expect("seed");
        assert!(matches!(
            Identity::load_or_create_in(&entry),
            Err(LanBeamError::Crypto(_))
        ));
        assert_eq!(entry.get_secret().expect("still stored"), corrupt);
    }
}
