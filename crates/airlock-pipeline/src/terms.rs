//! Private terms sealed at rest. The 32-byte key comes from the OS keychain;
//! this module only does the seal, so tests never touch the keychain.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

const MAGIC: &[u8] = b"ALKT";
const VERSION: u8 = 1;

pub fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
    key
}

#[derive(Debug, thiserror::Error)]
pub enum TermsError {
    #[error("private-term seal is not readable")]
    BadSeal,
}

/// Encrypts the term list. The same terms seal to different bytes because the nonce is random.
pub fn seal_terms(terms: &[String], key: &[u8; 32]) -> Result<Vec<u8>, TermsError> {
    let json = serde_json::to_vec(terms).map_err(|_| TermsError::BadSeal)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| TermsError::BadSeal)?;
    let mut nonce_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), json.as_ref())
        .map_err(|_| TermsError::BadSeal)?;
    let mut out = Vec::with_capacity(5 + nonce_bytes.len() + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn open_terms(blob: &[u8], key: &[u8; 32]) -> Result<Vec<String>, TermsError> {
    if blob.len() < 5 + 12 || &blob[..4] != MAGIC || blob[4] != VERSION {
        return Err(TermsError::BadSeal);
    }
    let (nonce, ciphertext) = blob[5..].split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| TermsError::BadSeal)?;
    let json = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| TermsError::BadSeal)?;
    serde_json::from_slice(&json).map_err(|_| TermsError::BadSeal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_a_wrong_key_fails() {
        let key = [7u8; 32];
        let terms = vec!["Springfield High".into(), "Jane Doe".into()];
        let blob = seal_terms(&terms, &key).unwrap();
        assert!(!blob.windows(11).any(|w| w == b"Springfield"));
        assert_eq!(open_terms(&blob, &key).unwrap(), terms);
        let mut wrong = key;
        wrong[0] ^= 1;
        assert!(open_terms(&blob, &wrong).is_err());
    }
}
