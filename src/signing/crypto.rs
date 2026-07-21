use hmac::{Hmac, KeyInit as _, Mac};
use sha2::{Digest, Sha256};
#[cfg(test)]
use subtle::ConstantTimeEq as _;
use zeroize::Zeroize as _;

use super::SigningError;

type HmacSha256 = Hmac<Sha256>;

/// A derived `SigV4` key. It is cleared on drop and cannot be formatted.
pub(crate) struct SigningKey([u8; 32]);

impl SigningKey {
    pub(super) fn sign(&self, message: &[u8]) -> Result<[u8; 32], SigningError> {
        hmac_sha256(&self.0, message)
    }

    #[cfg(test)]
    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Compute a payload SHA-256 digest.
pub(crate) fn payload_sha256(payload: &[u8]) -> [u8; 32] {
    Sha256::digest(payload).into()
}

/// Compute a lowercase hexadecimal payload SHA-256 digest.
pub(crate) fn payload_sha256_hex(payload: &[u8]) -> String {
    hex_encode(&payload_sha256(payload))
}

/// Derive the scoped `SigV4` signing key.
pub(crate) fn derive_signing_key(
    secret_access_key: &[u8],
    date: &str,
    region: &str,
    service: &str,
) -> Result<SigningKey, SigningError> {
    let mut root = Vec::with_capacity(4 + secret_access_key.len());
    root.extend_from_slice(b"AWS4");
    root.extend_from_slice(secret_access_key);

    let mut date_key = hmac_sha256(&root, date.as_bytes())?;
    root.zeroize();
    let mut region_key = hmac_sha256(&date_key, region.as_bytes())?;
    date_key.zeroize();
    let mut service_key = hmac_sha256(&region_key, service.as_bytes())?;
    region_key.zeroize();
    let signing_key = hmac_sha256(&service_key, b"aws4_request")?;
    service_key.zeroize();

    Ok(SigningKey(signing_key))
}

/// Compare two encoded signatures without content-dependent early exit.
#[cfg(test)]
pub(crate) fn constant_time_signature_eq(expected: &str, candidate: &str) -> bool {
    let expected_bytes = expected.as_bytes();
    let candidate_bytes = candidate.as_bytes();
    let same_length = expected_bytes.len().ct_eq(&candidate_bytes.len());

    // SigV4 signatures are fixed-size lowercase hex. Comparing a fixed local
    // buffer avoids returning early based on a candidate byte mismatch.
    let mut expected_fixed = [0_u8; 64];
    let mut candidate_fixed = [0_u8; 64];
    let expected_copy = expected_bytes.len().min(expected_fixed.len());
    let candidate_copy = candidate_bytes.len().min(candidate_fixed.len());
    expected_fixed[..expected_copy].copy_from_slice(&expected_bytes[..expected_copy]);
    candidate_fixed[..candidate_copy].copy_from_slice(&candidate_bytes[..candidate_copy]);

    bool::from(same_length & expected_fixed.ct_eq(&candidate_fixed))
        && expected_bytes.len() == expected_fixed.len()
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> Result<[u8; 32], SigningError> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| SigningError::Cryptographic)?;
    mac.update(message);
    Ok(mac.finalize().into_bytes().into())
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_empty_payload_using_aws_vector() {
        assert_eq!(
            payload_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn derives_deterministic_scoped_signing_key() {
        let key = derive_signing_key(
            b"wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20120215",
            "us-east-1",
            "iam",
        )
        .unwrap();
        assert_eq!(
            hex_encode(key.as_bytes()),
            "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d"
        );
    }

    #[test]
    fn signature_comparison_requires_canonical_length_and_exact_bytes() {
        let signature = "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7";
        assert!(constant_time_signature_eq(signature, signature));
        assert!(!constant_time_signature_eq(
            signature,
            "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d0"
        ));
        assert!(!constant_time_signature_eq(signature, "5d672d79"));
        assert!(!constant_time_signature_eq("", ""));
    }
}
