use std::{collections::BTreeMap, fmt, num::NonZeroU16};

use http::HeaderMap;

use super::{
    ByteRange, Checksum, ChecksumAlgorithm, ChecksumType, Conditions, ObjectKey, RequestIds,
};
use crate::stream::{ByteStream, ResponseStream};

/// Metadata common to object retrieval and inspection responses.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObjectMetadata {
    /// Entity tag returned by the service.
    pub e_tag: Option<String>,
    /// Object size in bytes.
    pub content_length: u64,
    /// Object media type.
    pub content_type: Option<String>,
    /// Object modification time.
    pub last_modified: Option<time::OffsetDateTime>,
    /// Version identifier, when bucket versioning is enabled.
    pub version_id: Option<String>,
    /// Caller-defined `x-amz-meta-*` values.
    pub user_metadata: BTreeMap<String, String>,
    /// Checksums supplied by the service.
    pub checksum: Checksum,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// Request to upload one object.
#[derive(derive_more::Debug)]
pub struct PutObjectRequest {
    /// Destination object key.
    pub key: ObjectKey,
    /// Upload body. Replayability is determined by its selected source.
    #[debug("{:?}", "<stream>")]
    pub body: ByteStream,
    /// Optional media type.
    pub content_type: Option<String>,
    /// Caller-defined object metadata.
    #[debug("{:?}", self.user_metadata.keys().collect::<Vec<_>>())]
    pub user_metadata: BTreeMap<String, String>,
    /// Preconditions for the write.
    pub conditions: Conditions,
    /// Ask S3 to calculate or validate this checksum algorithm.
    pub checksum_algorithm: Option<ChecksumAlgorithm>,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl PutObjectRequest {
    /// Constructs a request with no optional headers.
    pub fn new(key: ObjectKey, body: ByteStream) -> Self {
        Self {
            key,
            body,
            content_type: None,
            user_metadata: BTreeMap::new(),
            conditions: Conditions::default(),
            checksum_algorithm: None,
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// Result of uploading one object.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PutObjectOutput {
    /// Entity tag assigned by the service.
    pub e_tag: Option<String>,
    /// Version identifier, when enabled.
    pub version_id: Option<String>,
    /// Returned checksum values.
    pub checksum: Checksum,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// Request to download one object.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct GetObjectRequest {
    /// Object key.
    pub key: ObjectKey,
    /// Optional byte range.
    pub range: Option<ByteRange>,
    /// Optional preconditions.
    pub conditions: Conditions,
    /// Specific object version to retrieve.
    pub version_id: Option<String>,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl GetObjectRequest {
    /// Constructs a full-object request without preconditions.
    pub fn new(key: ObjectKey) -> Self {
        Self {
            key,
            range: None,
            conditions: Conditions::default(),
            version_id: None,
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// A streaming object download and its response metadata.
pub struct GetObjectOutput {
    /// Response metadata parsed before the body is consumed.
    pub metadata: ObjectMetadata,
    /// Streaming response body.
    pub body: ResponseStream,
    /// Inclusive range returned for a ranged request.
    pub content_range: Option<(u64, u64, Option<u64>)>,
}

impl fmt::Debug for GetObjectOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GetObjectOutput")
            .field("metadata", &self.metadata)
            .field("body", &"<stream>")
            .field("content_range", &self.content_range)
            .finish()
    }
}

/// Request to inspect object metadata without retrieving the body.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct HeadObjectRequest {
    /// Object key.
    pub key: ObjectKey,
    /// Optional preconditions.
    pub conditions: Conditions,
    /// Specific object version to inspect.
    pub version_id: Option<String>,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl HeadObjectRequest {
    /// Constructs an unconditional request for the latest version.
    pub fn new(key: ObjectKey) -> Self {
        Self {
            key,
            conditions: Conditions::default(),
            version_id: None,
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// Result of inspecting an object.
pub type HeadObjectOutput = ObjectMetadata;

/// Request to delete one object.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct DeleteObjectRequest {
    /// Object key.
    pub key: ObjectKey,
    /// Specific version to remove.
    pub version_id: Option<String>,
    /// Optional entity-tag precondition.
    pub if_match: Option<String>,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl DeleteObjectRequest {
    /// Constructs a request for the latest object version.
    pub fn new(key: ObjectKey) -> Self {
        Self {
            key,
            version_id: None,
            if_match: None,
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// Result of deleting one object.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteObjectOutput {
    /// Whether the response represents a delete marker.
    pub delete_marker: bool,
    /// Removed version identifier, when present.
    pub version_id: Option<String>,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// One object selected for a multi-object delete request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteObjectIdentifier {
    /// Object key.
    pub key: ObjectKey,
    /// Specific version to remove.
    pub version_id: Option<String>,
    /// Optional entity-tag precondition.
    pub if_match: Option<String>,
}

impl DeleteObjectIdentifier {
    /// Constructs an identifier for the latest object version.
    pub fn new(key: ObjectKey) -> Self {
        Self {
            key,
            version_id: None,
            if_match: None,
        }
    }
}

/// Maximum number of entries accepted by S3's multi-object delete API.
pub const MAX_DELETE_OBJECTS: usize = 1_000;

/// A validated multi-object delete request.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct DeleteObjectsRequest {
    objects: Vec<DeleteObjectIdentifier>,
    /// Suppresses per-key success entries when true.
    pub quiet: bool,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl DeleteObjectsRequest {
    /// Validates that the batch contains between one and 1,000 entries.
    pub fn new(objects: Vec<DeleteObjectIdentifier>) -> Result<Self, DeleteObjectsError> {
        if objects.is_empty() {
            return Err(DeleteObjectsError::Empty);
        }
        if objects.len() > MAX_DELETE_OBJECTS {
            return Err(DeleteObjectsError::TooMany {
                actual: objects.len(),
                maximum: MAX_DELETE_OBJECTS,
            });
        }
        Ok(Self {
            objects,
            quiet: false,
            headers: HeaderMap::new(),
        })
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Returns the validated delete entries.
    pub fn objects(&self) -> &[DeleteObjectIdentifier] {
        &self.objects
    }
}

/// Invalid multi-object delete batch.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DeleteObjectsError {
    /// The batch has no entries.
    #[error("a multi-object delete request cannot be empty")]
    Empty,
    /// The batch exceeds the S3 limit.
    #[error("multi-object delete has {actual} entries; the maximum is {maximum}")]
    TooMany {
        /// Actual entry count.
        actual: usize,
        /// Maximum accepted entry count.
        maximum: usize,
    },
}

/// One successfully deleted object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeletedObject {
    /// Deleted key.
    pub key: ObjectKey,
    /// Deleted version identifier.
    pub version_id: Option<String>,
    /// Whether a delete marker was created or removed.
    pub delete_marker: bool,
    /// Delete-marker version identifier.
    pub delete_marker_version_id: Option<String>,
}

/// Per-object failure returned from a multi-object delete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteError {
    /// Key that was not deleted. Invalid server keys are retained as text.
    pub key: String,
    /// Version identifier, when present.
    pub version_id: Option<String>,
    /// S3 error code.
    pub code: Option<String>,
    /// Service-supplied diagnostic message.
    pub message: Option<String>,
}

/// Result of a multi-object delete request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteObjectsOutput {
    /// Successfully deleted entries.
    pub deleted: Vec<DeletedObject>,
    /// Entries the service failed to delete.
    pub errors: Vec<DeleteError>,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// Source of a server-side object copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopySource {
    /// Source bucket name.
    pub bucket: String,
    /// Source object key.
    pub key: ObjectKey,
    /// Source version identifier.
    pub version_id: Option<String>,
}

/// Metadata behavior for a server-side copy.
///
/// S3's `REPLACE` directive replaces the complete metadata set. Keeping that
/// choice explicit prevents changing one header from accidentally discarding
/// all source user metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum CopyMetadataDirective {
    /// Copy the source object's media type and user metadata unchanged.
    #[default]
    Copy,
    /// Replace the complete metadata set with the supplied values.
    Replace {
        /// Replacement media type. When omitted, S3 applies its default.
        content_type: Option<String>,
        /// Complete replacement set of caller-defined metadata.
        user_metadata: BTreeMap<String, String>,
    },
}

/// Request to copy an object within S3.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct CopyObjectRequest {
    /// Copy source.
    pub source: CopySource,
    /// Destination object key in the configured bucket.
    pub destination: ObjectKey,
    /// Preconditions evaluated against the source object.
    pub source_conditions: Conditions,
    /// Whether to copy or completely replace source metadata.
    #[debug(
        "{:?}",
        match self.metadata {
            CopyMetadataDirective::Copy => "copy",
            CopyMetadataDirective::Replace { .. } => "replace",
        }
    )]
    pub metadata: CopyMetadataDirective,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl CopyObjectRequest {
    /// Constructs an unconditional copy that preserves all source metadata.
    pub fn new(source: CopySource, destination: ObjectKey) -> Self {
        Self {
            source,
            destination,
            source_conditions: Conditions::default(),
            metadata: CopyMetadataDirective::Copy,
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// Owner information optionally returned for a listed object.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObjectOwner {
    /// Canonical account identifier.
    pub id: Option<String>,
    /// Legacy display name, when the service still supplies one.
    pub display_name: Option<String>,
}

/// Result of a server-side copy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopyObjectOutput {
    /// Entity tag of the copied object.
    pub e_tag: Option<String>,
    /// Modification time reported by the service.
    pub last_modified: Option<time::OffsetDateTime>,
    /// Destination version identifier.
    pub version_id: Option<String>,
    /// Returned checksum values.
    pub checksum: Checksum,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// Validated S3 listing page size in the range 1 through 1,000.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageSize(NonZeroU16);

impl PageSize {
    /// Maximum page size accepted by S3 listing operations.
    pub const MAX: u16 = 1_000;

    /// Constructs a page size if it is in the supported range.
    pub fn new(value: u16) -> Option<Self> {
        NonZeroU16::new(value)
            .filter(|value| value.get() <= Self::MAX)
            .map(Self)
    }

    /// Returns the validated value.
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

impl Default for PageSize {
    fn default() -> Self {
        // `MAX` is statically non-zero.
        Self(NonZeroU16::new(Self::MAX).expect("page-size maximum must be non-zero"))
    }
}

/// Request for one `ListObjectsV2` page.
#[derive(Clone, Default, derive_more::Debug, Eq, PartialEq)]
pub struct ListObjectsV2Request {
    /// Only keys beginning with this exact prefix are returned.
    pub prefix: Option<String>,
    /// Groups keys using this delimiter.
    pub delimiter: Option<String>,
    /// Opaque token from a preceding response.
    pub continuation_token: Option<String>,
    /// Starts after this key when no continuation token is used.
    pub start_after: Option<ObjectKey>,
    /// Maximum entries requested from the service.
    pub max_keys: PageSize,
    /// Requests owner information for every entry.
    pub fetch_owner: bool,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl ListObjectsV2Request {
    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// One object returned by `ListObjectsV2`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedObject {
    /// Object key.
    pub key: ObjectKey,
    /// Modification time.
    pub last_modified: Option<time::OffsetDateTime>,
    /// Entity tag.
    pub e_tag: Option<String>,
    /// Object size in bytes.
    pub size: u64,
    /// Storage class, when supplied.
    pub storage_class: Option<String>,
    /// Owner information requested with `fetch_owner`.
    pub owner: Option<ObjectOwner>,
    /// Checksum algorithms associated with this object.
    pub checksum_algorithms: Vec<String>,
    /// Whether reported checksums represent the full object or a composite.
    pub checksum_type: Option<ChecksumType>,
}

/// One parsed `ListObjectsV2` page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListObjectsV2Output {
    /// Objects in the page.
    pub objects: Vec<ListedObject>,
    /// Grouped key prefixes.
    pub common_prefixes: Vec<String>,
    /// Whether another page exists.
    pub is_truncated: bool,
    /// Opaque token for retrieving the next page.
    pub next_continuation_token: Option<String>,
    /// Number of keys represented in this page.
    pub key_count: Option<u32>,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_batches_are_strictly_bounded() {
        assert_eq!(
            DeleteObjectsRequest::new(Vec::new()).unwrap_err(),
            DeleteObjectsError::Empty
        );
        let key = ObjectKey::new("key").unwrap();
        let too_many = (0..=MAX_DELETE_OBJECTS)
            .map(|_| DeleteObjectIdentifier::new(key.clone()))
            .collect();
        assert!(matches!(
            DeleteObjectsRequest::new(too_many),
            Err(DeleteObjectsError::TooMany { .. })
        ));
    }

    #[test]
    fn custom_header_values_are_redacted() {
        let mut request = GetObjectRequest::new(ObjectKey::new("key").unwrap());
        request.headers.insert(
            "x-amz-server-side-encryption-customer-key",
            "sentinel-sse-c-key".parse().unwrap(),
        );
        let debug = format!("{request:?}");
        assert!(debug.contains(r#"headers: "<redacted>""#));
        assert!(!debug.contains("sentinel-sse-c-key"));
    }

    #[test]
    fn page_sizes_reject_zero_and_values_over_service_limit() {
        assert!(PageSize::new(0).is_none());
        assert_eq!(PageSize::new(1).unwrap().get(), 1);
        assert_eq!(PageSize::new(1_000).unwrap().get(), 1_000);
        assert!(PageSize::new(1_001).is_none());
    }
}
