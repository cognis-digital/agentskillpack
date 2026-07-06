//! Detached ed25519 signatures over `.skillpack` archives (provenance).
//!
//! # Trust model (read this honestly)
//!
//! This is **TOFU / bring-your-own-key**, not a PKI. There is no certificate
//! authority, no key transparency log, and no revocation. A signature proves
//! only that *whoever holds the private key* signed *these exact bytes*. The
//! host decides which public keys it trusts (typically by pinning them out of
//! band). What signing buys you over an unsigned archive:
//!
//! - **Integrity**: any change to the archive bytes invalidates the signature
//!   (this is stronger than, and independent of, the per-file SHA-256 in the
//!   header — it covers the whole container including the header).
//! - **Provenance**: a verifier holding the signer's public key can confirm the
//!   archive came from that signer and was not swapped for another.
//!
//! What it does **not** buy you: it does not tell you the signer is trustworthy,
//! nor that the key wasn't stolen. That is the host's key-management problem.
//!
//! # What is signed
//!
//! The signature is computed over the **entire archive byte stream** (magic,
//! version, header, and all blobs). Because the header already binds every
//! file's SHA-256, signing the container transitively signs the file contents.
//!
//! # On-disk artifacts
//!
//! - Keys are stored as 64 lowercase hex characters (32 raw bytes) in a text
//!   file, so they are diff-friendly and copy-pasteable.
//! - A signature sidecar is a small JSON document ([`SignatureFile`]) with the
//!   algorithm, signer public key, and hex signature. It lives next to the
//!   archive as `<archive>.sig`.

use std::fs;
use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Algorithm label recorded in signature sidecars.
pub const ALGORITHM: &str = "ed25519";
/// Suffix appended to an archive path to form its signature sidecar path.
pub const SIG_SUFFIX: &str = ".sig";

/// A generated keypair, held as raw 32-byte halves.
pub struct KeyPair {
    signing: SigningKey,
}

impl KeyPair {
    /// Generate a fresh random keypair from the OS CSPRNG.
    pub fn generate() -> Self {
        KeyPair {
            signing: SigningKey::generate(&mut OsRng),
        }
    }

    /// The private (signing) key as 64 hex chars.
    pub fn private_hex(&self) -> String {
        to_hex(&self.signing.to_bytes())
    }

    /// The public (verifying) key as 64 hex chars.
    pub fn public_hex(&self) -> String {
        to_hex(&self.signing.verifying_key().to_bytes())
    }

    /// Reconstruct a keypair from a 64-hex private key.
    pub fn from_private_hex(hex: &str) -> Result<Self> {
        let bytes = from_hex(hex.trim(), "private key")?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Manifest("private key must be 32 bytes (64 hex chars)".into()))?;
        Ok(KeyPair {
            signing: SigningKey::from_bytes(&arr),
        })
    }

    /// Sign `message`, returning the 64-byte signature as 128 hex chars.
    pub fn sign_hex(&self, message: &[u8]) -> String {
        to_hex(&self.signing.sign(message).to_bytes())
    }

    /// Build a full [`SignatureFile`] sidecar for `archive_bytes`.
    pub fn sign_archive(&self, archive_bytes: &[u8]) -> SignatureFile {
        SignatureFile {
            algorithm: ALGORITHM.to_string(),
            public_key: self.public_hex(),
            signature: self.sign_hex(archive_bytes),
        }
    }
}

/// A detached signature sidecar (`<archive>.sig`), serialized as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureFile {
    /// Signature algorithm (currently always `ed25519`).
    pub algorithm: String,
    /// Signer's public key, 64 hex chars.
    pub public_key: String,
    /// The detached signature, 128 hex chars.
    pub signature: String,
}

impl SignatureFile {
    /// Serialize to pretty JSON with a trailing newline.
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("signature serializes");
        s.push('\n');
        s
    }

    /// Parse a sidecar from JSON bytes.
    pub fn from_slice(raw: &[u8]) -> Result<SignatureFile> {
        serde_json::from_slice(raw)
            .map_err(|e| Error::Manifest(format!("signature sidecar is not valid: {e}")))
    }

    /// Verify this sidecar against `archive_bytes`.
    ///
    /// Optionally require the sidecar's `public_key` to equal `expected_pubkey`
    /// (a pinned, trusted key). Returns `Ok(())` only when the algorithm is
    /// supported, the (optional) key matches, and the signature validates over
    /// the exact archive bytes.
    pub fn verify(&self, archive_bytes: &[u8], expected_pubkey: Option<&str>) -> Result<()> {
        if self.algorithm != ALGORITHM {
            return Err(Error::Integrity(format!(
                "unsupported signature algorithm '{}'",
                self.algorithm
            )));
        }
        if let Some(expected) = expected_pubkey {
            if !keys_equal(expected, &self.public_key) {
                return Err(Error::Integrity(
                    "signature was made by a different key than the one provided".into(),
                ));
            }
        }
        let vk = verifying_key_from_hex(&self.public_key)?;
        let sig = signature_from_hex(&self.signature)?;
        vk.verify(archive_bytes, &sig)
            .map_err(|_| Error::Integrity("signature does not match archive bytes".into()))
    }
}

/// Verify raw pieces directly (used by tests and the registry): given the
/// archive bytes, a hex public key, and a hex signature, confirm the signature.
pub fn verify_raw(archive_bytes: &[u8], public_key_hex: &str, signature_hex: &str) -> Result<()> {
    let vk = verifying_key_from_hex(public_key_hex)?;
    let sig = signature_from_hex(signature_hex)?;
    vk.verify(archive_bytes, &sig)
        .map_err(|_| Error::Integrity("signature does not match archive bytes".into()))
}

/// Compute the sidecar path for an archive path (`foo.skillpack` ->
/// `foo.skillpack.sig`).
pub fn sig_path_for(archive: &Path) -> std::path::PathBuf {
    let mut s = archive.as_os_str().to_os_string();
    s.push(SIG_SUFFIX);
    std::path::PathBuf::from(s)
}

/// Write a keypair to `<dir>/<name>.key` (private) and `<dir>/<name>.pub`
/// (public). Returns the two paths.
pub fn write_keypair(
    kp: &KeyPair,
    dir: &Path,
    name: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    fs::create_dir_all(dir)?;
    let priv_path = dir.join(format!("{name}.key"));
    let pub_path = dir.join(format!("{name}.pub"));
    fs::write(&priv_path, format!("{}\n", kp.private_hex()))?;
    fs::write(&pub_path, format!("{}\n", kp.public_hex()))?;
    Ok((priv_path, pub_path))
}

fn verifying_key_from_hex(hex: &str) -> Result<VerifyingKey> {
    let bytes = from_hex(hex.trim(), "public key")?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Integrity("public key must be 32 bytes (64 hex chars)".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| Error::Integrity(format!("invalid public key: {e}")))
}

fn signature_from_hex(hex: &str) -> Result<Signature> {
    let bytes = from_hex(hex.trim(), "signature")?;
    let arr: [u8; 64] = bytes
        .try_into()
        .map_err(|_| Error::Integrity("signature must be 64 bytes (128 hex chars)".into()))?;
    Ok(Signature::from_bytes(&arr))
}

/// Constant-length string compare for public keys (not timing-sensitive here,
/// but avoids surprises).
fn keys_equal(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(hex: &str, what: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return Err(Error::Integrity(format!("{what}: odd-length hex string")));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i]).ok_or_else(|| Error::Integrity(format!("{what}: bad hex")))?;
        let lo =
            hex_val(bytes[i + 1]).ok_or_else(|| Error::Integrity(format!("{what}: bad hex")))?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keygen_produces_64_hex_keys() {
        let kp = KeyPair::generate();
        assert_eq!(kp.private_hex().len(), 64);
        assert_eq!(kp.public_hex().len(), 64);
        assert!(kp.private_hex().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let kp = KeyPair::generate();
        let archive = b"pretend this is a .skillpack byte stream";
        let sig = kp.sign_archive(archive);
        assert_eq!(sig.algorithm, ALGORITHM);
        sig.verify(archive, None).unwrap();
        sig.verify(archive, Some(&kp.public_hex())).unwrap();
    }

    #[test]
    fn tampered_archive_fails_verification() {
        let kp = KeyPair::generate();
        let archive = b"original bytes".to_vec();
        let sig = kp.sign_archive(&archive);
        let mut tampered = archive.clone();
        tampered[0] ^= 0xff;
        assert!(sig.verify(&tampered, None).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let signer = KeyPair::generate();
        let attacker = KeyPair::generate();
        let archive = b"payload";
        let sig = signer.sign_archive(archive);
        // Pinning the attacker's key must reject even though bytes are intact.
        let err = sig
            .verify(archive, Some(&attacker.public_hex()))
            .unwrap_err();
        assert!(matches!(err, Error::Integrity(_)));
    }

    #[test]
    fn forged_signature_from_other_key_fails() {
        let signer = KeyPair::generate();
        let attacker = KeyPair::generate();
        let archive = b"payload";
        // Attacker signs the same bytes but the sidecar claims the signer's key.
        let mut forged = attacker.sign_archive(archive);
        forged.public_key = signer.public_hex();
        assert!(forged.verify(archive, None).is_err());
    }

    #[test]
    fn keypair_reconstructs_from_private_hex() {
        let kp = KeyPair::generate();
        let restored = KeyPair::from_private_hex(&kp.private_hex()).unwrap();
        assert_eq!(kp.public_hex(), restored.public_hex());
        let archive = b"data";
        restored
            .sign_archive(archive)
            .verify(archive, Some(&kp.public_hex()))
            .unwrap();
    }

    #[test]
    fn sidecar_json_roundtrips() {
        let kp = KeyPair::generate();
        let sig = kp.sign_archive(b"x");
        let parsed = SignatureFile::from_slice(sig.to_json().as_bytes()).unwrap();
        assert_eq!(sig, parsed);
    }

    #[test]
    fn unsupported_algorithm_is_rejected() {
        let kp = KeyPair::generate();
        let mut sig = kp.sign_archive(b"x");
        sig.algorithm = "rsa".into();
        assert!(sig.verify(b"x", None).is_err());
    }

    #[test]
    fn bad_hex_is_rejected() {
        assert!(from_hex("zz", "test").is_err());
        assert!(from_hex("abc", "test").is_err());
    }

    #[test]
    fn sig_path_appends_suffix() {
        let p = sig_path_for(Path::new("foo.skillpack"));
        assert!(p.to_string_lossy().ends_with("foo.skillpack.sig"));
    }
}
