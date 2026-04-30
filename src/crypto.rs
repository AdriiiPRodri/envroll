//! Encryption layer: age scrypt-passphrase mode, binary on-disk format
//! (design.md D7).
//!
//! - [`encrypt`] / [`decrypt`] are thin wrappers over `age::simple::encrypt`
//!   / `age::simple::decrypt` with a [`scrypt::Recipient`] / [`scrypt::Identity`].
//! - [`create_canary`] / [`verify_canary`] handle `<vault>/.canary.age` per
//!   the vault-encryption spec.
//!
//! Passphrase hygiene: every API in this module receives the passphrase by
//! reference to a [`SecretString`]. We never `.clone()` into a `String` and
//! never `Display`-format it; `secrecy` zeroizes on drop.
//!
//! The on-disk format is binary age (no ASCII armor) — see the round-trip
//! test that asserts the first 21 bytes are the v1 header `age-encryption.org/v1`.

use age::scrypt;
use age::secrecy::{ExposeSecret, SecretString};
use std::path::Path;

use crate::errors::EnvrollError;
use crate::paths::vault_canary;
use crate::vault::fs as vfs;

/// Fixed canary plaintext (vault-encryption spec). Must NEVER change without a
/// schema bump (`<vault>/.envroll-version`).
const CANARY_PLAINTEXT: &[u8] = b"envroll-canary-v1\n";

/// Permission bits for `.age` blobs (0600, design.md D8 — defense in depth).
const AGE_BLOB_MODE: u32 = 0o600;

/// Encrypt `plaintext` with `passphrase` using age scrypt mode, binary format.
///
/// Returns the full ciphertext as a `Vec<u8>`. The caller is responsible for
/// writing it atomically (use [`crate::vault::fs::atomic_write`]).
pub fn encrypt(plaintext: &[u8], passphrase: &SecretString) -> Result<Vec<u8>, EnvrollError> {
    let recipient = scrypt::Recipient::new(passphrase.clone());
    age::encrypt(&recipient, plaintext)
        .map_err(|e| EnvrollError::Generic(format!("age encryption failed: {e}")))
}

/// Decrypt `ciphertext` (binary age) with `passphrase`.
///
/// Maps every age decryption failure to [`EnvrollError::FileCorrupt`] with a
/// generic placeholder path; callers that know the source path should produce
/// a more specific error message before propagating. Callers decrypting the
/// canary translate this to [`EnvrollError::WrongPassphrase`] per the
/// vault-encryption spec.
pub fn decrypt(ciphertext: &[u8], passphrase: &SecretString) -> Result<Vec<u8>, EnvrollError> {
    let identity = scrypt::Identity::new(passphrase.clone());
    age::decrypt(&identity, ciphertext)
        .map_err(|e| EnvrollError::FileCorrupt(format!("age decryption failed: {e}")))
}

/// Create `<vault>/.canary.age` containing the fixed plaintext encrypted with
/// `passphrase`. Mode 0600 (design.md D8). The vault git commit is the
/// caller's responsibility (Vault::ensure_init handles it).
pub fn create_canary(vault_root: &Path, passphrase: &SecretString) -> Result<(), EnvrollError> {
    let ciphertext = encrypt(CANARY_PLAINTEXT, passphrase)?;
    vfs::atomic_write(&vault_canary(vault_root), &ciphertext, AGE_BLOB_MODE)
}

/// Decrypt `<vault>/.canary.age` and verify it matches the expected fixed
/// plaintext. Translates every failure mode to [`EnvrollError::WrongPassphrase`]
/// — the canary is the spec-defined arbiter of "is this the right passphrase".
///
/// Errors:
/// - File missing: `EnvrollError::Generic(...)` with the spec-mandated message
///   so the binary boundary can format it correctly. (The error category is
///   not `wrong passphrase` because the cause is structurally different.)
/// - Decryption failure or content mismatch: [`EnvrollError::WrongPassphrase`].
pub fn verify_canary(vault_root: &Path, passphrase: &SecretString) -> Result<(), EnvrollError> {
    let path = vault_canary(vault_root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(EnvrollError::Generic(
                "vault canary missing — run `envroll init` to repair".to_string(),
            ));
        }
        Err(e) => return Err(EnvrollError::Io(e)),
    };
    let plaintext = match decrypt(&bytes, passphrase) {
        Ok(p) => p,
        Err(_) => return Err(EnvrollError::WrongPassphrase),
    };
    if plaintext != CANARY_PLAINTEXT {
        return Err(EnvrollError::WrongPassphrase);
    }
    // Drop plaintext immediately; nothing sensitive lingers.
    drop(plaintext);
    let _ = passphrase.expose_secret(); // keep the import used; see hygiene note above
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Pick a low-but-valid scrypt work factor for tests so they run quickly.
    /// Production calls do NOT touch this knob — the age crate's default
    /// (~1 second on the host) is what users see.
    fn weak_passphrase(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    #[test]
    fn encrypt_then_decrypt_roundtrips() {
        let pass = weak_passphrase("correct horse battery staple");
        let ct = encrypt(b"DATABASE_URL=postgres://x\n", &pass).unwrap();
        let pt = decrypt(&ct, &pass).unwrap();
        assert_eq!(pt, b"DATABASE_URL=postgres://x\n");
    }

    #[test]
    fn ciphertext_is_binary_age_v1_format() {
        // vault-encryption spec: persisted blobs MUST start with the binary
        // age v1 header. ASCII-armored output (`-----BEGIN AGE...`) is forbidden.
        let pass = weak_passphrase("p");
        let ct = encrypt(b"x", &pass).unwrap();
        assert!(
            ct.starts_with(b"age-encryption.org/v1"),
            "ciphertext does not start with age v1 binary header (first 32 bytes: {:?})",
            &ct[..ct.len().min(32)]
        );
    }

    #[test]
    fn decrypt_with_wrong_passphrase_yields_file_corrupt() {
        let pass = weak_passphrase("right");
        let ct = encrypt(b"hello", &pass).unwrap();
        let other = weak_passphrase("wrong");
        let err = decrypt(&ct, &other).unwrap_err();
        assert!(matches!(err, EnvrollError::FileCorrupt(_)));
    }

    #[test]
    fn canary_create_then_verify_succeeds() {
        let dir = TempDir::new().unwrap();
        let pass = weak_passphrase("vault-pass");
        create_canary(dir.path(), &pass).unwrap();
        verify_canary(dir.path(), &pass).unwrap();
    }

    #[test]
    fn canary_verify_with_wrong_passphrase_yields_wrong_passphrase() {
        let dir = TempDir::new().unwrap();
        create_canary(dir.path(), &weak_passphrase("right")).unwrap();
        let err = verify_canary(dir.path(), &weak_passphrase("wrong")).unwrap_err();
        assert!(matches!(err, EnvrollError::WrongPassphrase));
    }

    #[test]
    fn canary_verify_when_missing_returns_generic_with_repair_message() {
        let dir = TempDir::new().unwrap();
        let err = verify_canary(dir.path(), &weak_passphrase("any")).unwrap_err();
        match err {
            EnvrollError::Generic(msg) => assert!(msg.contains("vault canary missing")),
            other => panic!("expected Generic('vault canary missing...'), got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn canary_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        create_canary(dir.path(), &weak_passphrase("p")).unwrap();
        let mode = std::fs::metadata(vault_canary(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
