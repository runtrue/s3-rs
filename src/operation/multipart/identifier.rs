use std::{fmt, str::FromStr};

/// An opaque, validated multipart upload identifier issued by an S3 service.
///
/// Upload identifiers may grant the ability to add parts to or abort an
/// in-progress upload. Formatting therefore always redacts the value; use
/// [`UploadId::as_str`] or [`UploadId::expose`] when the wire value is needed.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UploadId(String);

impl UploadId {
    /// Maximum accepted UTF-8 byte length for an upload identifier.
    ///
    /// S3 treats this value as opaque. The bound prevents untrusted service
    /// responses from becoming unbounded query parameters while leaving ample
    /// room for identifiers produced by S3-compatible implementations.
    pub const MAX_LENGTH: usize = 2_048;

    /// Validates and stores an upload identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, UploadIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(UploadIdError::Empty);
        }
        if value.len() > Self::MAX_LENGTH {
            return Err(UploadIdError::TooLong {
                length: value.len(),
                maximum: Self::MAX_LENGTH,
            });
        }
        if let Some(index) = value.bytes().position(|byte| byte.is_ascii_control()) {
            return Err(UploadIdError::AsciiControl { index });
        }
        Ok(Self(value))
    }

    /// Explicitly exposes the identifier for protocol and query construction.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Returns the identifier for protocol and query construction.
    pub fn as_str(&self) -> &str {
        self.expose()
    }
}

impl fmt::Debug for UploadId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UploadId([REDACTED])")
    }
}

impl fmt::Display for UploadId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl FromStr for UploadId {
    type Err = UploadIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for UploadId {
    type Error = UploadIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Why a service-issued multipart upload identifier was rejected.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum UploadIdError {
    /// The identifier had no bytes.
    #[error("multipart upload ID cannot be empty")]
    Empty,
    /// The identifier exceeded [`UploadId::MAX_LENGTH`].
    #[error("multipart upload ID is {length} bytes; the maximum is {maximum}")]
    TooLong {
        /// Actual UTF-8 byte length.
        length: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum: usize,
    },
    /// The identifier contained a potentially log- or query-confusing ASCII control.
    #[error("multipart upload ID contains an ASCII control at byte {index}")]
    AsciiControl {
        /// Byte offset of the first ASCII control.
        index: usize,
    },
}
