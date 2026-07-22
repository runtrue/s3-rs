use std::{fmt, num::NonZeroU16};

use super::{MultipartError, UploadId};
use crate::operation::{Checksum, ObjectKey, RequestIds};
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
pub struct UploadPartRequest {
    key: ObjectKey,
    upload_id: UploadId,
    part_number: PartNumber,
    body: ByteStream,
    checksum: Checksum,
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

impl fmt::Debug for UploadPartRequest {
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
