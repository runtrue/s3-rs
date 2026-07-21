use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use bytes::Bytes;

use super::{CompletedPart, MultipartError, UploadId};
use crate::operation::{ChecksumAlgorithm, ObjectKey, RequestIds};

/// Request to initiate a multipart upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateMultipartUploadRequest {
    /// Destination object key.
    pub key: ObjectKey,
    /// Optional object media type.
    pub content_type: Option<String>,
    /// User-defined object metadata.
    pub user_metadata: BTreeMap<String, String>,
    /// Checksum algorithm applied to uploaded parts.
    pub checksum_algorithm: Option<ChecksumAlgorithm>,
}

impl CreateMultipartUploadRequest {
    /// Constructs a request without optional metadata.
    pub fn new(key: ObjectKey) -> Self {
        Self {
            key,
            content_type: None,
            user_metadata: BTreeMap::new(),
            checksum_algorithm: None,
        }
    }
}

/// Result of initiating a multipart upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateMultipartUploadOutput {
    /// Bucket reported by the service.
    pub bucket: Option<String>,
    /// Object key reported by the service.
    pub key: ObjectKey,
    /// Opaque upload identifier required by later operations.
    upload_id: UploadId,
    /// Checksum algorithm selected by the service.
    pub checksum_algorithm: Option<String>,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

impl CreateMultipartUploadOutput {
    pub(crate) fn new(
        bucket: Option<String>,
        key: ObjectKey,
        upload_id: UploadId,
        checksum_algorithm: Option<String>,
    ) -> Self {
        Self {
            bucket,
            key,
            upload_id,
            checksum_algorithm,
            request_ids: RequestIds::default(),
        }
    }

    /// Returns the validated upload identifier required by later operations.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }
}

/// Explicit state needed to resume or clean up an in-progress multipart upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultipartUpload {
    key: ObjectKey,
    upload_id: UploadId,
    /// Parts that have completed successfully, in ascending order.
    completed_parts: Vec<CompletedPart>,
}

pub(crate) enum MultipartUploadSource {
    Bytes(Bytes),
    File(PathBuf),
}

/// Request for a bounded, automatically cleaned-up multipart upload.
///
/// In-memory sources retain the caller's complete byte buffer for the duration
/// of the operation. File sources use only the configured in-flight part byte
/// budget in memory; their immutable snapshot is disk-backed.
pub struct ManagedMultipartUploadRequest {
    pub(crate) key: ObjectKey,
    pub(crate) source: MultipartUploadSource,
    pub(crate) content_type: Option<String>,
    pub(crate) user_metadata: BTreeMap<String, String>,
}

impl ManagedMultipartUploadRequest {
    /// Constructs a request with a replayable in-memory body.
    pub fn from_bytes(key: ObjectKey, bytes: impl Into<Bytes>) -> Self {
        Self {
            key,
            source: MultipartUploadSource::Bytes(bytes.into()),
            content_type: None,
            user_metadata: BTreeMap::new(),
        }
    }

    /// Constructs a request with a replayable regular-file body.
    ///
    /// The file is copied into a private disk-backed snapshot before S3 creates
    /// the multipart upload, so later changes to the source path have no effect.
    pub fn from_path(key: ObjectKey, path: impl AsRef<Path>) -> Self {
        Self {
            key,
            source: MultipartUploadSource::File(path.as_ref().to_owned()),
            content_type: None,
            user_metadata: BTreeMap::new(),
        }
    }

    /// Sets the object's media type.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Adds user-defined object metadata.
    pub fn with_metadata(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.user_metadata.insert(name.into(), value.into());
        self
    }

    /// Returns the destination object key.
    pub const fn key(&self) -> &ObjectKey {
        &self.key
    }
}

impl fmt::Debug for ManagedMultipartUploadRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedMultipartUploadRequest")
            .field("key", &self.key)
            .field(
                "source",
                &match self.source {
                    MultipartUploadSource::Bytes(_) => "bytes",
                    MultipartUploadSource::File(_) => "file",
                },
            )
            .field("content_type", &self.content_type)
            .field(
                "user_metadata_names",
                &self.user_metadata.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl MultipartUpload {
    /// Starts tracking a newly created upload.
    pub fn new(key: ObjectKey, upload_id: UploadId) -> Self {
        Self {
            key,
            upload_id,
            completed_parts: Vec::new(),
        }
    }

    /// Records a completed part while retaining canonical ascending order.
    pub fn record_part(&mut self, part: CompletedPart) -> Result<(), MultipartError> {
        match self
            .completed_parts
            .binary_search_by_key(&part.part_number(), CompletedPart::part_number)
        {
            Ok(_) => Err(MultipartError::DuplicatePart(part.part_number())),
            Err(index) => {
                self.completed_parts.insert(index, part);
                Ok(())
            }
        }
    }

    /// Returns completed parts in validated ascending order.
    pub fn completed_parts(&self) -> &[CompletedPart] {
        &self.completed_parts
    }

    /// Returns the destination object key.
    pub const fn key(&self) -> &ObjectKey {
        &self.key
    }

    /// Returns the non-empty service-issued upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }
}
