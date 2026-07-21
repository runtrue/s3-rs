use super::UploadId;
use crate::operation::{ObjectKey, PageSize, RequestIds};

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
