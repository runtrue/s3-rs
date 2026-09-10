//! Typed requests, responses, and values for S3 operations.

mod checksum;
mod multipart;
mod object;
mod types;

pub use checksum::ChecksumCalculationError;
pub(crate) use multipart::MultipartUploadSource;
pub use multipart::{
    AbortMultipartUploadRequest, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest,
    CompletedPart, CopyPartRange, CreateMultipartUploadOutput, CreateMultipartUploadRequest,
    ListMultipartUploadsOutput, ListMultipartUploadsRequest, ListPartsOutput, ListPartsRequest,
    ListedPart, ManagedMultipartHeaders, ManagedMultipartUploadRequest, MultipartError,
    MultipartOptions, MultipartUpload, MultipartUploadEntry, PartNumber, UploadId, UploadIdError,
    UploadPartCopyOutput, UploadPartCopyRequest, UploadPartOutput, UploadPartRequest,
};
pub use object::{
    CopyMetadataDirective, CopyObjectOutput, CopyObjectRequest, CopySource, DeleteError,
    DeleteObjectIdentifier, DeleteObjectOutput, DeleteObjectRequest, DeleteObjectsError,
    DeleteObjectsOutput, DeleteObjectsRequest, DeletedObject, GetObjectOutput, GetObjectRequest,
    HeadObjectOutput, HeadObjectRequest, ListObjectsV2Output, ListObjectsV2Request, ListedObject,
    ObjectMetadata, ObjectOwner, PageSize, PutObjectOutput, PutObjectRequest,
};
pub use types::{
    ByteRange, Checksum, ChecksumAlgorithm, ChecksumType, Conditions, ObjectKey, ObjectKeyError,
    PresignedUrl, RangeError, RequestIds,
};
