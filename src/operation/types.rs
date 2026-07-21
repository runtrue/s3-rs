use std::{fmt, num::NonZeroU64, str::FromStr};

use secrecy::{ExposeSecret, SecretString};

/// Maximum encoded byte length accepted by S3 for an object key.
pub const MAX_OBJECT_KEY_BYTES: usize = 1_024;

/// A validated UTF-8 S3 object key.
///
/// This type deliberately has no filesystem-path or URL semantics. A slash is
/// just another key byte and is preserved exactly.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectKey(String);

impl ObjectKey {
    /// Validates and constructs an object key.
    pub fn new(value: impl Into<String>) -> Result<Self, ObjectKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ObjectKeyError::Empty);
        }
        if value.len() > MAX_OBJECT_KEY_BYTES {
            return Err(ObjectKeyError::TooLong {
                actual: value.len(),
                maximum: MAX_OBJECT_KEY_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the key without interpreting or decoding it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the key and returns its UTF-8 representation.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for ObjectKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ObjectKey").field(&self.0).finish()
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for ObjectKey {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl TryFrom<String> for ObjectKey {
    type Error = ObjectKeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ObjectKey {
    type Error = ObjectKeyError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ObjectKey {
    type Err = ObjectKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Validation failure for an [`ObjectKey`].
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObjectKeyError {
    /// S3 object keys cannot be empty.
    #[error("an S3 object key cannot be empty")]
    Empty,
    /// The key exceeds S3's 1,024-byte limit.
    #[error("S3 object key is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Actual UTF-8 byte length.
        actual: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum: usize,
    },
}

/// A byte range for a GET request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteRange {
    /// Bytes from `start` through `end`, inclusive.
    Inclusive {
        /// First byte offset.
        start: u64,
        /// Last byte offset.
        end: u64,
    },
    /// All bytes starting at the given offset.
    From(u64),
    /// The last non-zero number of bytes.
    Suffix(NonZeroU64),
}

impl ByteRange {
    /// Constructs an inclusive range, rejecting an end before its start.
    pub fn inclusive(start: u64, end: u64) -> Result<Self, RangeError> {
        if end < start {
            return Err(RangeError { start, end });
        }
        Ok(Self::Inclusive { start, end })
    }

    /// Constructs a range extending from `start` to the end of the object.
    pub const fn from(start: u64) -> Self {
        Self::From(start)
    }

    /// Constructs a suffix range. Zero is rejected.
    pub fn suffix(length: u64) -> Option<Self> {
        NonZeroU64::new(length).map(Self::Suffix)
    }

    /// Returns the HTTP `Range` header value.
    pub fn to_header_value(self) -> String {
        match self {
            Self::Inclusive { start, end } => format!("bytes={start}-{end}"),
            Self::From(start) => format!("bytes={start}-"),
            Self::Suffix(length) => format!("bytes=-{length}"),
        }
    }
}

/// Invalid inclusive byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("range end {end} precedes start {start}")]
pub struct RangeError {
    /// First requested byte.
    pub start: u64,
    /// Last requested byte.
    pub end: u64,
}

/// Conditional headers shared by object operations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Conditions {
    /// Match this entity tag before performing the operation.
    pub if_match: Option<String>,
    /// Perform the operation only when this entity tag does not match.
    pub if_none_match: Option<String>,
    /// Perform the operation only if the object changed after this instant.
    pub if_modified_since: Option<time::OffsetDateTime>,
    /// Perform the operation only if the object did not change after this instant.
    pub if_unmodified_since: Option<time::OffsetDateTime>,
}

/// Checksum algorithm understood by S3 checksum headers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ChecksumAlgorithm {
    /// CRC-32.
    Crc32,
    /// CRC-32C.
    Crc32c,
    /// CRC-64/NVME.
    Crc64Nvme,
    /// SHA-1.
    Sha1,
    /// SHA-256.
    Sha256,
}

/// Base64-encoded checksums returned by S3.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Checksum {
    /// Base64-encoded CRC-32 value.
    pub crc32: Option<String>,
    /// Base64-encoded CRC-32C value.
    pub crc32c: Option<String>,
    /// Base64-encoded CRC-64/NVME value.
    pub crc64_nvme: Option<String>,
    /// Base64-encoded SHA-1 value.
    pub sha1: Option<String>,
    /// Base64-encoded SHA-256 value.
    pub sha256: Option<String>,
}

/// Identifiers supplied by an S3-compatible service for diagnostics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestIds {
    /// `x-amz-request-id`, when supplied.
    pub request_id: Option<String>,
    /// `x-amz-id-2`, when supplied.
    pub host_id: Option<String>,
}

/// A presigned URL whose standard formatting is always redacted.
///
/// Callers must explicitly opt in to exposing the URL because its query string
/// contains signing material.
#[derive(Clone)]
pub struct PresignedUrl(SecretString);

impl PresignedUrl {
    /// Wraps a generated presigned URL.
    pub(crate) fn new(url: impl Into<String>) -> Self {
        Self(url.into().into())
    }

    /// Explicitly exposes the full signed URL.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    /// Consumes this wrapper and explicitly exposes the full signed URL.
    pub fn into_exposed(self) -> String {
        self.0.expose_secret().to_owned()
    }
}

impl fmt::Debug for PresignedUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PresignedUrl([REDACTED])")
    }
}

impl fmt::Display for PresignedUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED PRESIGNED URL]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn byte_ranges_render_without_off_by_one_changes() {
        assert_eq!(
            ByteRange::inclusive(2, 9).unwrap().to_header_value(),
            "bytes=2-9"
        );
        assert_eq!(ByteRange::from(2).to_header_value(), "bytes=2-");
        assert_eq!(ByteRange::suffix(2).unwrap().to_header_value(), "bytes=-2");
        assert!(ByteRange::inclusive(9, 2).is_err());
        assert!(ByteRange::suffix(0).is_none());
    }

    #[test]
    fn presigned_url_formatting_is_redacted() {
        let signed = PresignedUrl::new("https://example.test/key?X-Amz-Signature=secret");
        assert!(!format!("{signed:?}").contains("secret"));
        assert!(!signed.to_string().contains("secret"));
        assert!(signed.expose().contains("secret"));
    }

    proptest! {
        #[test]
        fn valid_object_keys_round_trip(value in ".{1,300}") {
            prop_assume!(value.len() <= MAX_OBJECT_KEY_BYTES);
            let key = ObjectKey::new(value.clone()).unwrap();
            prop_assert_eq!(key.into_string(), value);
        }
    }
}
