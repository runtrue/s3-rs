use http::HeaderMap;

use super::{PartNumber, UploadId};
use crate::operation::{Checksum, ChecksumType, ObjectKey, PageSize, RequestIds};

/// Request for one page of in-progress multipart uploads.
#[derive(Clone, Default, derive_more::Debug, Eq, PartialEq)]
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
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl ListMultipartUploadsRequest {
    /// Replaces the request's additional headers.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
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

/// Request for one page of parts belonging to an in-progress upload.
#[derive(Clone, derive_more::Debug, Eq, PartialEq)]
pub struct ListPartsRequest {
    key: ObjectKey,
    upload_id: UploadId,
    /// Lists only parts after this part number.
    pub part_number_marker: Option<PartNumber>,
    /// Maximum parts requested from the service.
    pub max_parts: PageSize,
    /// Additional request headers. Values are signed and repeated values are preserved.
    /// Generated-name collisions are errors; signing- and transport-owned headers are rejected.
    #[debug("{:?}", "<redacted>")]
    pub headers: HeaderMap,
}

impl ListPartsRequest {
    /// Constructs a request for the first page of an upload's parts.
    pub fn new(key: ObjectKey, upload_id: UploadId) -> Self {
        Self {
            key,
            upload_id,
            part_number_marker: None,
            max_parts: PageSize::default(),
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

    /// Returns the service-issued upload identifier.
    pub const fn upload_id(&self) -> &UploadId {
        &self.upload_id
    }
}

/// One uploaded part returned by `ListParts`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedPart {
    part_number: PartNumber,
    /// Entity tag used when completing the upload.
    pub e_tag: String,
    /// Time at which S3 accepted the part.
    pub last_modified: Option<time::OffsetDateTime>,
    /// Part size in bytes.
    pub size: u64,
    /// Checksums recorded for this part.
    pub checksum: Checksum,
}

impl ListedPart {
    pub(crate) fn new(
        part_number: PartNumber,
        e_tag: String,
        last_modified: Option<time::OffsetDateTime>,
        size: u64,
        checksum: Checksum,
    ) -> Self {
        Self {
            part_number,
            e_tag,
            last_modified,
            size,
            checksum,
        }
    }

    /// Returns the validated part number.
    pub const fn part_number(&self) -> PartNumber {
        self.part_number
    }

    /// Converts this service entry into a completion descriptor.
    pub fn completed_part(&self) -> Result<super::CompletedPart, super::MultipartError> {
        super::CompletedPart::new(self.part_number.get(), self.e_tag.clone())
            .map(|part| part.with_checksum(self.checksum.clone()))
    }
}

/// One bounded page returned by `ListParts`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListPartsOutput {
    /// Parts in ascending part-number order.
    pub parts: Vec<ListedPart>,
    /// Whether another page exists.
    pub is_truncated: bool,
    /// Marker to use for the next page.
    pub next_part_number_marker: Option<PartNumber>,
    /// Checksum algorithm selected when the upload was created.
    pub checksum_algorithm: Option<String>,
    /// Object-level checksum aggregation selected for the upload.
    pub checksum_type: Option<ChecksumType>,
    /// Service request identifiers.
    pub request_ids: RequestIds,
}
