//! Encryption for upstream provider credentials at rest.
//!
//! Provider API keys added at runtime are sealed with AES-256-GCM before they
//! reach the `provider_keys` table. The key-encryption key (KEK) comes from
//! the `ROLTER_KEK` environment variable — any sufficiently random string;
//! the 256-bit cipher key is derived from it with SHA-256, so operators are
//! not forced into a specific encoding or length. Ciphertext and the random
//! 96-bit nonce are stored side by side; the KEK never touches the database.

use aes_gcm::aead::{Aead, Generate};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use sha2::{Digest, Sha256};

use rolter_core::{Error, Result};

/// Environment variable holding the key-encryption key.
pub const KEK_ENV: &str = "ROLTER_KEK";

/// A key-encryption key for sealing provider credentials.
#[derive(Clone)]
pub struct Kek([u8; 32]);

impl std::fmt::Debug for Kek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Kek(..)")
    }
}

impl Kek {
    /// Derive a KEK from an operator-supplied secret string.
    pub fn from_secret(secret: &str) -> Self {
        let digest = Sha256::digest(secret.as_bytes());
        Self(digest.into())
    }

    /// Read the KEK from [`KEK_ENV`]. Returns `None` when unset or empty.
    pub fn from_env() -> Option<Self> {
        match std::env::var(KEK_ENV) {
            Ok(secret) if !secret.trim().is_empty() => Some(Self::from_secret(&secret)),
            _ => None,
        }
    }

    /// Seal a plaintext credential. Returns `(ciphertext, nonce)` ready for
    /// the `provider_keys` table.
    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(self.0));
        let nonce = Nonce::<<Aes256Gcm as AeadCore>::NonceSize>::generate();
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| Error::Store("failed to encrypt provider key".into()))?;
        Ok((ciphertext, nonce.to_vec()))
    }

    /// Open a sealed credential. Fails when the KEK does not match the one
    /// that sealed it or the ciphertext was tampered with.
    pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<String> {
        let nonce = Nonce::<<Aes256Gcm as AeadCore>::NonceSize>::try_from(nonce)
            .map_err(|_| Error::Store("provider key nonce must be 12 bytes".into()))?;
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(self.0));
        let plaintext = cipher.decrypt(&nonce, ciphertext).map_err(|_| {
            Error::Store(format!(
                "failed to decrypt provider key; check that {KEK_ENV} matches the key \
                     used when the credential was stored"
            ))
        })?;
        String::from_utf8(plaintext)
            .map_err(|_| Error::Store("decrypted provider key is not valid utf-8".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_and_uses_fresh_nonces() {
        let kek = Kek::from_secret("correct horse battery staple");
        let (c1, n1) = kek.encrypt("sk-upstream-secret").unwrap();
        let (c2, n2) = kek.encrypt("sk-upstream-secret").unwrap();
        assert_ne!(n1, n2, "each encryption must draw a fresh nonce");
        assert_ne!(c1, c2);
        assert_eq!(kek.decrypt(&c1, &n1).unwrap(), "sk-upstream-secret");
        assert_eq!(kek.decrypt(&c2, &n2).unwrap(), "sk-upstream-secret");
    }

    #[test]
    fn wrong_kek_fails_closed() {
        let kek = Kek::from_secret("kek-a");
        let (ciphertext, nonce) = kek.encrypt("sk-upstream-secret").unwrap();
        let other = Kek::from_secret("kek-b");
        assert!(other.decrypt(&ciphertext, &nonce).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let kek = Kek::from_secret("kek");
        let (mut ciphertext, nonce) = kek.encrypt("sk-upstream-secret").unwrap();
        ciphertext[0] ^= 0xff;
        assert!(kek.decrypt(&ciphertext, &nonce).is_err());
    }
}
