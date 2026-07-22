use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use sha2::{Digest as _, Sha256};

use super::{Checksum, ChecksumAlgorithm};

/// Failure to calculate a requested checksum locally.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("local calculation for {algorithm:?} is not implemented")]
pub struct ChecksumCalculationError {
    /// Algorithm that could not be calculated.
    pub algorithm: ChecksumAlgorithm,
}

impl Checksum {
    /// Calculates one checksum over `bytes` using standard S3 wire encoding.
    ///
    /// CRC32, CRC32C, CRC64NVME, and SHA-256 are supported. SHA-1 remains an
    /// accepted wire value but is not calculated locally.
    ///
    /// # Errors
    ///
    /// Returns an error when local calculation is unavailable for `algorithm`.
    pub fn calculate(
        algorithm: ChecksumAlgorithm,
        bytes: &[u8],
    ) -> Result<Self, ChecksumCalculationError> {
        let mut checksum = Self::default();
        match algorithm {
            ChecksumAlgorithm::Crc32 => {
                checksum.crc32 = Some(BASE64_STANDARD.encode(crc32(bytes).to_be_bytes()));
            }
            ChecksumAlgorithm::Crc32c => {
                checksum.crc32c = Some(BASE64_STANDARD.encode(crc32c(bytes).to_be_bytes()));
            }
            ChecksumAlgorithm::Crc64Nvme => {
                checksum.crc64_nvme = Some(BASE64_STANDARD.encode(crc64_nvme(bytes).to_be_bytes()));
            }
            ChecksumAlgorithm::Sha256 => {
                checksum.sha256 = Some(BASE64_STANDARD.encode(Sha256::digest(bytes)));
            }
            ChecksumAlgorithm::Sha1 => return Err(ChecksumCalculationError { algorithm }),
        }
        Ok(checksum)
    }

    pub(crate) async fn calculate_cooperatively(
        algorithm: ChecksumAlgorithm,
        bytes: &[u8],
    ) -> Result<Self, ChecksumCalculationError> {
        const CHUNK_SIZE: usize = 256 * 1024;
        let mut checksum = Self::default();
        match algorithm {
            ChecksumAlgorithm::Crc32 | ChecksumAlgorithm::Crc32c => {
                let table = match algorithm {
                    ChecksumAlgorithm::Crc32 => &CRC32_TABLE,
                    ChecksumAlgorithm::Crc32c => &CRC32C_TABLE,
                    _ => unreachable!("matched CRC32 family"),
                };
                let mut value = u32::MAX;
                for chunk in bytes.chunks(CHUNK_SIZE) {
                    value = update_crc32_state(table, value, chunk);
                    tokio::task::yield_now().await;
                }
                let encoded = BASE64_STANDARD.encode((!value).to_be_bytes());
                match algorithm {
                    ChecksumAlgorithm::Crc32 => checksum.crc32 = Some(encoded),
                    ChecksumAlgorithm::Crc32c => checksum.crc32c = Some(encoded),
                    _ => unreachable!("matched CRC32 family"),
                }
            }
            ChecksumAlgorithm::Crc64Nvme => {
                let mut value = u64::MAX;
                for chunk in bytes.chunks(CHUNK_SIZE) {
                    value = update_crc64_state(value, chunk);
                    tokio::task::yield_now().await;
                }
                checksum.crc64_nvme = Some(BASE64_STANDARD.encode((!value).to_be_bytes()));
            }
            ChecksumAlgorithm::Sha256 => {
                let mut digest = Sha256::new();
                for chunk in bytes.chunks(CHUNK_SIZE) {
                    digest.update(chunk);
                    tokio::task::yield_now().await;
                }
                checksum.sha256 = Some(BASE64_STANDARD.encode(digest.finalize()));
            }
            ChecksumAlgorithm::Sha1 => return Err(ChecksumCalculationError { algorithm }),
        }
        Ok(checksum)
    }
}

const CRC32_TABLE: [u32; 256] = crc32_table(0xedb8_8320);
const CRC32C_TABLE: [u32; 256] = crc32_table(0x82f6_3b78);
const CRC64_NVME_TABLE: [u64; 256] = crc64_table(0x9a6c_9329_ac4b_c9b5);

fn crc32(bytes: &[u8]) -> u32 {
    update_crc32(&CRC32_TABLE, bytes)
}

fn crc32c(bytes: &[u8]) -> u32 {
    update_crc32(&CRC32C_TABLE, bytes)
}

fn update_crc32(table: &[u32; 256], bytes: &[u8]) -> u32 {
    !update_crc32_state(table, u32::MAX, bytes)
}

fn update_crc32_state(table: &[u32; 256], mut value: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        let index = usize::from((value as u8) ^ byte);
        value = table[index] ^ (value >> 8);
    }
    value
}

fn crc64_nvme(bytes: &[u8]) -> u64 {
    !update_crc64_state(u64::MAX, bytes)
}

fn update_crc64_state(mut value: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        let index = usize::from((value as u8) ^ byte);
        value = CRC64_NVME_TABLE[index] ^ (value >> 8);
    }
    value
}

const fn crc32_table(polynomial: u32) -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0;
    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 0 {
                value >> 1
            } else {
                (value >> 1) ^ polynomial
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const fn crc64_table(polynomial: u64) -> [u64; 256] {
    let mut table = [0_u64; 256];
    let mut index = 0;
    while index < table.len() {
        let mut value = index as u64;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 0 {
                value >> 1
            } else {
                (value >> 1) ^ polynomial
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_crc_check_values_match() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
        assert_eq!(crc64_nvme(b"123456789"), 0xae8b_1486_0a79_9888);
    }

    #[test]
    fn calculates_s3_base64_values() {
        assert_eq!(
            Checksum::calculate(ChecksumAlgorithm::Crc64Nvme, b"123456789")
                .unwrap()
                .crc64_nvme
                .as_deref(),
            Some("rosUhgp5mIg=")
        );
        assert_eq!(
            Checksum::calculate(ChecksumAlgorithm::Sha256, b"abc")
                .unwrap()
                .sha256
                .as_deref(),
            Some("ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=")
        );
    }

    #[tokio::test]
    async fn cooperative_calculation_matches_synchronous_vectors() {
        let bytes = vec![0x5a; 1024 * 1024 + 17];
        for algorithm in [
            ChecksumAlgorithm::Crc32,
            ChecksumAlgorithm::Crc32c,
            ChecksumAlgorithm::Crc64Nvme,
            ChecksumAlgorithm::Sha256,
        ] {
            assert_eq!(
                Checksum::calculate_cooperatively(algorithm, &bytes)
                    .await
                    .unwrap(),
                Checksum::calculate(algorithm, &bytes).unwrap()
            );
        }
    }
}
