//! Device identity: one long-term X25519 static keypair. The 32-byte public key
//! IS the Device ID. Generated on first run via `snow`'s resolver, persisted as a
//! single 64-byte `public||private` blob in the OS keychain. The private key never
//! crosses the IPC boundary.

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

pub struct Identity {
    pub public: [u8; 32],
    private: Zeroizing<[u8; 32]>,
}

impl Identity {
    /// Load the persisted identity, or generate + store one on first run.
    /// `account_suffix` gives a distinct keychain identity per test instance
    /// (so two instances on one machine don't share a Device ID).
    pub fn load_or_create(account_suffix: Option<&str>) -> Result<Identity> {
        let entry = Entry::new(KR_SERVICE, &account_for(account_suffix))?;
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
            _ => {
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

/// Delete the persisted identity from the OS keychain (M5.7 reset). A missing
/// entry counts as success: the goal state — "no stored identity, the next
/// launch generates a fresh one" — is already true, and a retried reset must
/// not fail because its own first attempt half-succeeded.
pub fn delete_persisted(account_suffix: Option<&str>) -> Result<()> {
    let entry = Entry::new(KR_SERVICE, &account_for(account_suffix))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
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

    #[test]
    fn generated_keys_have_correct_shape() {
        let id = generate_identity().expect("generate");
        assert_eq!(id.public.len(), 32);
        assert_eq!(id.private_bytes().len(), 32);
        assert_eq!(id.device_id().len(), 43); // base64url no-pad of 32 bytes
    }

    // AC-K3: a second load returns the SAME identity (persisted in the OS keychain).
    // Hits the real credential store; the entry is the same one the app uses.
    #[test]
    fn identity_is_stable_across_loads() {
        let a = Identity::load_or_create(None).expect("first load");
        let b = Identity::load_or_create(None).expect("second load");
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
        let a = Identity::load_or_create(Some(&sfx)).expect("create");
        delete_persisted(Some(&sfx)).expect("delete existing entry");
        delete_persisted(Some(&sfx)).expect("deleting a missing entry is success");
        let b = Identity::load_or_create(Some(&sfx)).expect("regenerate");
        assert_ne!(
            a.device_id(),
            b.device_id(),
            "reset must yield a fresh identity"
        );
        // leave nothing behind in the user's credential store
        delete_persisted(Some(&sfx)).expect("cleanup");
    }
}
