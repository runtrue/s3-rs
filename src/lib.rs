//! Async, streaming S3-compatible client with explicit resource bounds.
//!
//! `s3-wire` provides typed object operations, SigV4 signing, replay-aware
//! retries, streaming downloads, and primitive or managed multipart uploads.
//! A client is configured for one endpoint, region, and bucket.
//!
//! # Quick start
//!
//! The default credential provider reads `AWS_ACCESS_KEY_ID`,
//! `AWS_SECRET_ACCESS_KEY`, and optional `AWS_SESSION_TOKEN`.
//!
//! ```no_run
//! use s3_wire::{
//!     ByteStream, Endpoint, GetObjectRequest, ObjectKey, PutObjectRequest, S3Client, S3Config,
//! };
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let region = "us-east-1";
//! let config = S3Config::builder()
//!     .endpoint(Endpoint::for_aws_region(region)?)
//!     .region(region)
//!     .bucket("artifact-bucket")
//!     .build()?;
//! let client = S3Client::new(config)?;
//!
//! let key = ObjectKey::new("reports/latest.json")?;
//! client
//!     .put_object(PutObjectRequest::new(
//!         key.clone(),
//!         ByteStream::from_bytes(br#"{"status":"complete"}"#.as_slice()),
//!     ))
//!     .await?;
//!
//! let download = client.get_object(GetObjectRequest::new(key)).await?;
//! let mut destination = tokio::io::sink();
//! download.body.write_to(&mut destination).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Upload and download behavior
//!
//! - [`ByteStream::from_bytes`] and [`ByteStream::from_path`] are replayable.
//! - [`ByteStream::from_stream`] is one-shot and requires an exact length and
//!   SHA-256 digest.
//! - [`ResponseStream`] applies backpressure and validates declared length,
//!   configured deadlines, and supported checksums while the body is consumed.
//! - [`S3Client::multipart_upload`] bounds concurrent part buffers and owns
//!   abort cleanup after S3 creates an upload.
//!
//! # Errors
//!
//! [`S3Error`] exposes a stable [`ErrorCategory`], service status and code,
//! request identifiers, retry classification, timeout phase, and any multipart
//! cleanup failure. Formatting remains redacted by default.
//!
//! # More documentation
//!
//! - [Architecture](https://github.com/runtrue/s3-rs/blob/main/docs/architecture.md)
//! - [S3 compatibility](https://github.com/runtrue/s3-rs/blob/main/docs/compatibility.md)
//! - [Security model](https://github.com/runtrue/s3-rs/blob/main/docs/security-model.md)
//! - [Examples](https://github.com/runtrue/s3-rs/blob/main/examples/README.md)

#![forbid(unsafe_code)]

mod client;
mod config;
mod credentials;
mod endpoint;
mod error;
mod operation;
mod retry;
mod stream;

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;

mod protocol;
mod signing;
mod transport;

pub use client::S3Client;
pub use config::{AddressingStyle, S3Config, S3ConfigBuilder};
pub use credentials::{
    CachedCredentialsProvider, Credentials, CredentialsProvider, EnvironmentCredentialsProvider,
    StaticCredentialsProvider,
};
pub use endpoint::Endpoint;
pub use error::{ErrorCategory, RetryClassification, S3Error, TimeoutPhase};
pub use operation::{
    AbortMultipartUploadRequest, ByteRange, Checksum, ChecksumAlgorithm,
    CompleteMultipartUploadOutput, CompleteMultipartUploadRequest, CompletedPart, Conditions,
    CopyObjectOutput, CopyObjectRequest, CopySource, CreateMultipartUploadOutput,
    CreateMultipartUploadRequest, DeleteError, DeleteObjectOutput, DeleteObjectRequest,
    DeleteObjectsError, DeleteObjectsOutput, DeleteObjectsRequest, DeletedObject, GetObjectOutput,
    GetObjectRequest, HeadObjectOutput, HeadObjectRequest, ListMultipartUploadsOutput,
    ListMultipartUploadsRequest, ListObjectsV2Output, ListObjectsV2Request, ListedObject,
    ManagedMultipartUploadRequest, MultipartError, MultipartOptions, MultipartUpload,
    MultipartUploadEntry, ObjectKey, ObjectKeyError, ObjectMetadata, PageSize, PartNumber,
    PresignedUrl, PutObjectOutput, PutObjectRequest, RangeError, RequestIds, UploadId,
    UploadIdError, UploadPartOutput, UploadPartRequest,
};
pub use retry::RetryPolicy;
pub use stream::{ByteStream, ResponseStream};
