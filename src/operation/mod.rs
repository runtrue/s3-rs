//! Typed requests, responses, and values for S3 operations.

mod multipart;
mod object;
mod types;

pub(crate) use multipart::MultipartUploadSource;
pub use multipart::{
    AbortMultipartUploadRequest, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest,
    CompletedPart, CreateMultipartUploadOutput, CreateMultipartUploadRequest,
    ListMultipartUploadsOutput, ListMultipartUploadsRequest, ManagedMultipartUploadRequest,
    MultipartError, MultipartOptions, MultipartUpload, MultipartUploadEntry, PartNumber, UploadId,
    UploadIdError, UploadPartOutput, UploadPartRequest,
};
pub use object::{
    CopyObjectOutput, CopyObjectRequest, CopySource, DeleteError, DeleteObjectOutput,
    DeleteObjectRequest, DeleteObjectsError, DeleteObjectsOutput, DeleteObjectsRequest,
    DeletedObject, GetObjectOutput, GetObjectRequest, HeadObjectOutput, HeadObjectRequest,
    ListObjectsV2Output, ListObjectsV2Request, ListedObject, ObjectMetadata, PageSize,
    PutObjectOutput, PutObjectRequest,
};
pub use types::{
    ByteRange, Checksum, ChecksumAlgorithm, Conditions, ObjectKey, ObjectKeyError, PresignedUrl,
    RangeError, RequestIds,
};
