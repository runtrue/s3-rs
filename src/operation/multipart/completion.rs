use http::HeaderMap;

use super::{CompletedPart, PartNumber, UploadId};
use crate::operation::{Checksum, ObjectKey, RequestIds};

/// Request to complete a multipart upload.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct CompleteMultipartUploadRequest {
    /// Destination object key.
    pub key: ObjectKey,
    upload_id: UploadId,
    completed_parts: Vec<CompletedPart>,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl CompleteMultipartUploadRequest {
    /// Validates that the upload ID is present and parts are non-empty, unique,
    /// and strictly ascending.
    pub fn new(
        key: ObjectKey,
        upload_id: UploadId,
        completed_parts: Vec<CompletedPart>,
    ) -> Result<Self, MultipartError> {
        validate_completed_parts(&completed_parts)?;
        Ok(Self {
            key,
            upload_id,
            completed_parts,
            headers: HeaderMap::new(),
        })
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Returns the validated, ordered completed parts.
    pub fn completed_parts(&self) -> &[CompletedPart] {
        &self.completed_parts
    }

    /// Returns the validated, non-empty upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }
}

/// Result of completing a multipart upload.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompleteMultipartUploadOutput {
    /// Service-provided object location. This is not exposed as a signed URL.
    pub location: Option<String>,
    /// Bucket reported by the service.
    pub bucket: Option<String>,
    /// Completed object key.
    pub key: Option<ObjectKey>,
    /// Entity tag of the completed object.
    pub e_tag: Option<String>,
    /// Version identifier, when enabled.
    pub version_id: Option<String>,
    /// Checksums returned by the service.
    pub checksum: Checksum,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// Request to abort an in-progress multipart upload.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct AbortMultipartUploadRequest {
    key: ObjectKey,
    upload_id: UploadId,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl AbortMultipartUploadRequest {
    /// Constructs an abort request from a validated upload identifier.
    pub fn new(key: ObjectKey, upload_id: UploadId) -> Self {
        Self {
            key,
            upload_id,
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Returns the destination object key.
    pub const fn key(&self) -> &ObjectKey {
        &self.key
    }

    /// Returns the validated multipart upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }
}

/// Invalid multipart state or descriptor.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MultipartError {
    /// A part number is outside S3's 1 through 10,000 range.
    #[error("multipart part number {0} is outside 1..=10000")]
    InvalidPartNumber(u16),
    /// An uploaded part has no usable entity tag.
    #[error("multipart part {part_number} has an empty ETag")]
    EmptyETag {
        /// Part with the invalid entity tag.
        part_number: u16,
    },
    /// A completed part number was recorded more than once.
    #[error("multipart part {0:?} was recorded more than once")]
    DuplicatePart(PartNumber),
    /// Completion was attempted without any completed parts.
    #[error("a multipart upload cannot be completed without parts")]
    NoCompletedParts,
    /// Completed parts are duplicated or out of ascending order.
    #[error("multipart part {next:?} does not follow {previous:?} in ascending order")]
    OutOfOrder {
        /// Last accepted part number.
        previous: PartNumber,
        /// Rejected part number.
        next: PartNumber,
    },
}

fn validate_completed_parts(parts: &[CompletedPart]) -> Result<(), MultipartError> {
    if parts.is_empty() {
        return Err(MultipartError::NoCompletedParts);
    }
    for window in parts.windows(2) {
        if window[1].part_number() <= window[0].part_number() {
            return Err(MultipartError::OutOfOrder {
                previous: window[0].part_number(),
                next: window[1].part_number(),
            });
        }
    }
    Ok(())
}
