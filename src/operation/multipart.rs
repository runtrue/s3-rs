use std::path::{Path, PathBuf};
use std::{fmt, num::NonZeroU16, str::FromStr};

use bytes::Bytes;

use super::{Checksum, ChecksumAlgorithm, ObjectKey, PageSize, RequestIds};

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

/// A validated multipart part number in the range 1 through 10,000.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PartNumber(NonZeroU16);

impl PartNumber {
    /// Highest part number accepted by S3.
    pub const MAX: u16 = 10_000;

    /// Constructs a part number in S3's supported range.
    pub fn new(value: u16) -> Option<Self> {
        NonZeroU16::new(value)
            .filter(|value| value.get() <= Self::MAX)
            .map(Self)
    }

    /// Returns the validated number.
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

/// Request to initiate a multipart upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateMultipartUploadRequest {
    /// Destination object key.
    pub key: ObjectKey,
    /// Optional object media type.
    pub content_type: Option<String>,
    /// User-defined object metadata.
    pub user_metadata: std::collections::BTreeMap<String, String>,
    /// Checksum algorithm applied to uploaded parts.
    pub checksum_algorithm: Option<ChecksumAlgorithm>,
}

impl CreateMultipartUploadRequest {
    /// Constructs a request without optional metadata.
    pub fn new(key: ObjectKey) -> Self {
        Self {
            key,
            content_type: None,
            user_metadata: std::collections::BTreeMap::new(),
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
    pub(crate) user_metadata: std::collections::BTreeMap<String, String>,
}

impl ManagedMultipartUploadRequest {
    /// Constructs a request with a replayable in-memory body.
    pub fn from_bytes(key: ObjectKey, bytes: impl Into<Bytes>) -> Self {
        Self {
            key,
            source: MultipartUploadSource::Bytes(bytes.into()),
            content_type: None,
            user_metadata: std::collections::BTreeMap::new(),
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
            user_metadata: std::collections::BTreeMap::new(),
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
    ///
    /// Concurrent uploads may finish in any order. Duplicate part numbers are
    /// rejected, while unique parts are inserted into their completion order.
    pub fn record_part(&mut self, part: CompletedPart) -> Result<(), MultipartError> {
        match self
            .completed_parts
            .binary_search_by_key(&part.part_number, |completed| completed.part_number)
        {
            Ok(_) => Err(MultipartError::DuplicatePart(part.part_number)),
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

/// Request to upload one multipart part.
pub struct UploadPartRequest<B> {
    /// Destination object key.
    key: ObjectKey,
    /// Opaque multipart upload identifier.
    upload_id: UploadId,
    /// Validated part number.
    part_number: PartNumber,
    /// Part body.
    body: B,
    /// Optional checksum of the part.
    checksum: Checksum,
}

impl<B> UploadPartRequest<B> {
    /// Constructs an upload-part request from validated identifiers.
    pub fn new(key: ObjectKey, upload_id: UploadId, part_number: PartNumber, body: B) -> Self {
        Self {
            key,
            upload_id,
            part_number,
            body,
            checksum: Checksum::default(),
        }
    }

    /// Returns the destination object key.
    pub const fn key(&self) -> &ObjectKey {
        &self.key
    }

    /// Returns the validated multipart upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }

    /// Returns the validated part number.
    pub const fn part_number(&self) -> PartNumber {
        self.part_number
    }

    /// Returns the part body.
    pub const fn body(&self) -> &B {
        &self.body
    }

    /// Returns the part body mutably.
    pub fn body_mut(&mut self) -> &mut B {
        &mut self.body
    }

    /// Returns the optional checksum of the part.
    pub const fn checksum(&self) -> &Checksum {
        &self.checksum
    }

    /// Attaches a checksum to the request.
    pub fn with_checksum(mut self, checksum: Checksum) -> Self {
        self.checksum = checksum;
        self
    }

    /// Consumes the request and returns its body.
    pub fn into_body(self) -> B {
        self.body
    }
}

impl<B> fmt::Debug for UploadPartRequest<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UploadPartRequest")
            .field("key", &self.key)
            .field("upload_id", &self.upload_id)
            .field("part_number", &self.part_number)
            .field("body", &"<stream>")
            .field("checksum", &self.checksum)
            .finish()
    }
}

/// Result of uploading one multipart part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadPartOutput {
    /// Part number supplied by the caller.
    pub part_number: PartNumber,
    /// Entity tag required when completing the upload.
    pub e_tag: String,
    /// Checksums returned by the service.
    pub checksum: Checksum,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

/// A validated completed-part descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedPart {
    part_number: PartNumber,
    e_tag: String,
    checksum: Checksum,
}

impl CompletedPart {
    /// Validates a part number and non-empty entity tag.
    pub fn new(part_number: u16, e_tag: impl Into<String>) -> Result<Self, MultipartError> {
        let part_number =
            PartNumber::new(part_number).ok_or(MultipartError::InvalidPartNumber(part_number))?;
        let e_tag = e_tag.into();
        if e_tag.trim().is_empty() {
            return Err(MultipartError::EmptyETag {
                part_number: part_number.get(),
            });
        }
        Ok(Self {
            part_number,
            e_tag,
            checksum: Checksum::default(),
        })
    }

    /// Returns the validated part number.
    pub const fn part_number(&self) -> PartNumber {
        self.part_number
    }

    /// Returns the non-empty entity tag.
    pub fn e_tag(&self) -> &str {
        &self.e_tag
    }

    /// Returns checksums associated with this part.
    pub const fn checksum(&self) -> &Checksum {
        &self.checksum
    }

    /// Attaches checksums returned by the part upload.
    pub fn with_checksum(mut self, checksum: Checksum) -> Self {
        self.checksum = checksum;
        self
    }
}

/// Request to complete a multipart upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteMultipartUploadRequest {
    /// Destination object key.
    pub key: ObjectKey,
    upload_id: UploadId,
    completed_parts: Vec<CompletedPart>,
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
        })
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbortMultipartUploadRequest {
    /// Destination object key.
    key: ObjectKey,
    /// Opaque multipart upload identifier.
    upload_id: UploadId,
}

impl AbortMultipartUploadRequest {
    /// Constructs an abort request from a validated upload identifier.
    pub fn new(key: ObjectKey, upload_id: UploadId) -> Self {
        Self { key, upload_id }
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

/// Request for one page of in-progress multipart uploads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListMultipartUploadsRequest {
    /// Only uploads with keys beginning with this prefix are returned.
    pub prefix: Option<String>,
    /// Groups upload keys using this delimiter.
    pub delimiter: Option<String>,
    /// Key marker from a preceding response.
    pub key_marker: Option<String>,
    /// Upload-ID marker from a preceding response.
    pub upload_id_marker: Option<UploadId>,
    /// Maximum uploads requested from the service.
    pub max_uploads: PageSize,
}

/// One in-progress multipart upload returned by the service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultipartUploadEntry {
    /// Destination object key.
    pub key: ObjectKey,
    /// Opaque upload identifier.
    upload_id: UploadId,
    /// Initiation time.
    pub initiated: Option<time::OffsetDateTime>,
    /// Storage class, when supplied.
    pub storage_class: Option<String>,
}

impl MultipartUploadEntry {
    pub(crate) fn new(
        key: ObjectKey,
        upload_id: UploadId,
        initiated: Option<time::OffsetDateTime>,
        storage_class: Option<String>,
    ) -> Self {
        Self {
            key,
            upload_id,
            initiated,
            storage_class,
        }
    }

    /// Returns the validated upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }
}

/// One page of in-progress multipart uploads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListMultipartUploadsOutput {
    /// Upload entries.
    pub uploads: Vec<MultipartUploadEntry>,
    /// Grouped key prefixes.
    pub common_prefixes: Vec<String>,
    /// Whether another page exists.
    pub is_truncated: bool,
    /// Key marker for the next page.
    pub next_key_marker: Option<String>,
    /// Upload-ID marker for the next page.
    pub next_upload_id_marker: Option<UploadId>,
    /// Service request identifiers.
    pub request_ids: RequestIds,
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
        if window[1].part_number <= window[0].part_number {
            return Err(MultipartError::OutOfOrder {
                previous: window[0].part_number,
                next: window[1].part_number,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_rejects_missing_duplicate_and_unordered_parts() {
        let key = ObjectKey::new("key").unwrap();
        let upload_id = UploadId::new("upload").unwrap();
        assert!(matches!(
            CompleteMultipartUploadRequest::new(key.clone(), upload_id.clone(), vec![]),
            Err(MultipartError::NoCompletedParts)
        ));

        let two = CompletedPart::new(2, "etag-2").unwrap();
        let duplicate = CompletedPart::new(2, "etag-2-again").unwrap();
        assert!(matches!(
            CompleteMultipartUploadRequest::new(
                key.clone(),
                upload_id.clone(),
                vec![two, duplicate]
            ),
            Err(MultipartError::OutOfOrder { .. })
        ));

        let two = CompletedPart::new(2, "etag-2").unwrap();
        let one = CompletedPart::new(1, "etag-1").unwrap();
        assert!(matches!(
            CompleteMultipartUploadRequest::new(key, upload_id, vec![two, one]),
            Err(MultipartError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn tracked_upload_sorts_concurrent_completion_order() {
        let mut upload = MultipartUpload::new(
            ObjectKey::new("key").unwrap(),
            UploadId::new("upload").unwrap(),
        );
        upload
            .record_part(CompletedPart::new(2, "etag-2").unwrap())
            .unwrap();
        upload
            .record_part(CompletedPart::new(1, "etag-1").unwrap())
            .unwrap();
        assert_eq!(upload.completed_parts()[0].part_number.get(), 1);
        assert_eq!(upload.completed_parts()[1].part_number.get(), 2);
        assert!(matches!(
            upload.record_part(CompletedPart::new(1, "duplicate").unwrap()),
            Err(MultipartError::DuplicatePart(_))
        ));
    }

    #[test]
    fn upload_id_rejects_empty_oversized_and_control_values() {
        assert_eq!(UploadId::new(""), Err(UploadIdError::Empty));
        assert_eq!(
            UploadId::new("x".repeat(UploadId::MAX_LENGTH + 1)),
            Err(UploadIdError::TooLong {
                length: UploadId::MAX_LENGTH + 1,
                maximum: UploadId::MAX_LENGTH,
            })
        );
        assert_eq!(
            UploadId::new("unsafe\nvalue"),
            Err(UploadIdError::AsciiControl { index: 6 })
        );
        assert_eq!(
            UploadId::new("unsafe\u{7f}value"),
            Err(UploadIdError::AsciiControl { index: 6 })
        );
        assert_eq!(
            UploadId::new("valid-opaque-üpload").unwrap().as_str(),
            "valid-opaque-üpload"
        );
    }

    #[test]
    fn upload_id_and_requests_redact_formatting() {
        let upload_id = UploadId::new("do-not-log-this-value").unwrap();
        assert_eq!(upload_id.to_string(), "[REDACTED]");
        assert_eq!(format!("{upload_id:?}"), "UploadId([REDACTED])");
        assert!(!format!("{upload_id:?}").contains(upload_id.as_str()));

        let request = UploadPartRequest::new(
            ObjectKey::new("key").unwrap(),
            upload_id,
            PartNumber::new(1).unwrap(),
            b"secret body".as_slice(),
        );
        let debug = format!("{request:?}");
        assert!(debug.contains("UploadId([REDACTED])"));
        assert!(!debug.contains("do-not-log-this-value"));
        assert!(!debug.contains("secret body"));
    }

    #[test]
    fn request_constructors_retain_validated_upload_ids() {
        let key = ObjectKey::new("key").unwrap();
        let upload_id = UploadId::new("upload").unwrap();
        let part = UploadPartRequest::new(
            key.clone(),
            upload_id.clone(),
            PartNumber::new(1).unwrap(),
            (),
        );
        let abort = AbortMultipartUploadRequest::new(key, upload_id);

        assert_eq!(part.upload_id().as_str(), "upload");
        assert_eq!(abort.upload_id().as_str(), "upload");
    }

    #[test]
    fn managed_request_debug_redacts_source_and_metadata_values() {
        let request = ManagedMultipartUploadRequest::from_path(
            ObjectKey::new("key").unwrap(),
            "/sensitive/source/path",
        )
        .with_metadata("name", "sensitive-value");
        let rendered = format!("{request:?}");
        assert!(rendered.contains("file"));
        assert!(rendered.contains("name"));
        assert!(!rendered.contains("/sensitive/source/path"));
        assert!(!rendered.contains("sensitive-value"));
    }
}
