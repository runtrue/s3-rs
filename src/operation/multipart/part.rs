use std::num::NonZeroU16;

use http::HeaderMap;

use super::{MultipartError, UploadId};
use crate::operation::{Checksum, Conditions, CopySource, ObjectKey, RequestIds};
use crate::stream::ByteStream;

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

/// Request to upload one multipart part.
#[derive(derive_more::Debug)]
pub struct UploadPartRequest {
    key: ObjectKey,
    upload_id: UploadId,
    part_number: PartNumber,
    #[debug("{:?}", "<stream>")]
    body: ByteStream,
    checksum: Checksum,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl UploadPartRequest {
    /// Constructs an upload-part request from validated identifiers.
    pub fn new(
        key: ObjectKey,
        upload_id: UploadId,
        part_number: PartNumber,
        body: ByteStream,
    ) -> Self {
        Self {
            key,
            upload_id,
            part_number,
            body,
            checksum: Checksum::default(),
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

    /// Returns the validated part number.
    pub const fn part_number(&self) -> PartNumber {
        self.part_number
    }

    /// Returns the part body.
    pub const fn body(&self) -> &ByteStream {
        &self.body
    }

    /// Returns the part body mutably.
    pub fn body_mut(&mut self) -> &mut ByteStream {
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
    pub fn into_body(self) -> ByteStream {
        self.body
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

/// Inclusive source byte range for one server-side copied part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyPartRange {
    start: u64,
    end: u64,
}

impl CopyPartRange {
    /// Constructs an inclusive range, rejecting an end before its start.
    pub fn new(start: u64, end: u64) -> Result<Self, crate::operation::RangeError> {
        if end < start {
            return Err(crate::operation::RangeError { start, end });
        }
        Ok(Self { start, end })
    }

    /// Returns the first copied byte offset.
    pub const fn start(self) -> u64 {
        self.start
    }

    /// Returns the final copied byte offset, inclusively.
    pub const fn end(self) -> u64 {
        self.end
    }

    pub(crate) fn header_value(self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }
}

/// Request to populate a multipart part from an existing S3 object.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct UploadPartCopyRequest {
    destination: ObjectKey,
    upload_id: UploadId,
    part_number: PartNumber,
    /// Object copied into this part.
    pub source: CopySource,
    /// Optional inclusive range within the source object.
    pub source_range: Option<CopyPartRange>,
    /// Preconditions evaluated against the source object.
    pub source_conditions: Conditions,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl UploadPartCopyRequest {
    /// Constructs a full-source copy request for one multipart part.
    pub fn new(
        destination: ObjectKey,
        upload_id: UploadId,
        part_number: PartNumber,
        source: CopySource,
    ) -> Self {
        Self {
            destination,
            upload_id,
            part_number,
            source,
            source_range: None,
            source_conditions: Conditions::default(),
            headers: HeaderMap::new(),
        }
    }

    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Returns the destination object key.
    pub const fn destination(&self) -> &ObjectKey {
        &self.destination
    }

    /// Returns the multipart upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }

    /// Returns the destination part number.
    pub const fn part_number(&self) -> PartNumber {
        self.part_number
    }
}

/// Result of copying an existing object or range into one multipart part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadPartCopyOutput {
    /// Destination part number supplied by the caller.
    pub part_number: PartNumber,
    /// Entity tag required when completing the upload.
    pub e_tag: String,
    /// Modification time reported for the copied part.
    pub last_modified: Option<time::OffsetDateTime>,
    /// Checksums returned for the copied part.
    pub checksum: Checksum,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}

impl UploadPartCopyOutput {
    /// Converts the successful result into a completion descriptor.
    pub fn completed_part(&self) -> Result<CompletedPart, MultipartError> {
        CompletedPart::new(self.part_number.get(), self.e_tag.clone())
            .map(|part| part.with_checksum(self.checksum.clone()))
    }
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
