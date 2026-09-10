//! A content-addressed, tenant-isolated artifact-store adapter.
//!
//! This mirrors the storage boundary a sandbox service needs without importing
//! application-specific types. The adapter keeps publication ordering,
//! integrity verification, cancellation, and garbage collection explicit.

use std::collections::BTreeSet;
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt as _;
use s3_wire::{
    AbortMultipartUploadRequest, AddressingStyle, ByteStream, Credentials, CredentialsProvider,
    DeleteObjectIdentifier, DeleteObjectRequest, DeleteObjectsRequest, Endpoint, ErrorCategory,
    GetObjectRequest, HeadObjectRequest, ListMultipartUploadsRequest, ListObjectsV2Request,
    ManagedMultipartUploadRequest, MultipartOptions, ObjectKey, ObjectKeyError, PutObjectRequest,
    S3Client, S3Config, S3Error, StaticCredentialsProvider, TimeoutPhase,
};
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use tokio::fs::File;
use tokio::io::{AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

const COPY_BUFFER_SIZE: usize = 64 * 1024;
const LISTING_PAGE_LIMIT: usize = 10_000;
const DELETE_BATCH_LIMIT: usize = 1_000;

#[derive(Debug, thiserror::Error)]
/// Failure returned by the example adapter.
pub enum ArtifactError {
    /// An S3 operation failed.
    #[error(transparent)]
    S3(#[from] S3Error),
    /// A derived object key exceeded S3 key constraints.
    #[error(transparent)]
    ObjectKey(#[from] ObjectKeyError),
    /// Reading, snapshotting, or writing local data failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A content identifier was not a canonical SHA-256 digest.
    #[error("SHA-256 digest must contain exactly 64 hexadecimal characters")]
    InvalidDigest,
    /// Downloaded or snapshotted bytes did not match their content identifier.
    #[error("downloaded artifact did not match its content digest")]
    DigestMismatch,
    /// The caller cancelled an in-progress multipart upload.
    #[error("multipart upload was cancelled")]
    Cancelled,
    /// The caller's multipart deadline expired.
    #[error("multipart upload exceeded its caller deadline")]
    DeadlineExceeded,
    /// A listing remained truncated after the configured number of pages.
    #[error("listing exceeded its configured page bound")]
    ListingPageLimit,
    /// Required process configuration was absent.
    #[error("required environment variable {0} is not set")]
    MissingEnvironment(&'static str),
}

/// A normalized lowercase SHA-256 content identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ContentDigest(String);

impl ContentDigest {
    /// Validates a 64-character hexadecimal SHA-256 digest.
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ArtifactError::InvalidDigest);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// Calculates the content identifier for an in-memory byte slice.
    pub fn of(bytes: &[u8]) -> Self {
        Self(hex_sha256(bytes))
    }

    /// Returns the lowercase hexadecimal digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reports which immutable objects were newly created by publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Publication {
    /// Whether this call created the payload blob.
    pub blob_created: bool,
    /// Whether this call created the manifest publication marker.
    pub manifest_created: bool,
}

/// Tenant-scoped, content-addressed artifact storage backed by S3.
#[derive(Clone)]
pub struct ArtifactStore {
    client: S3Client,
    tenant_prefix: String,
}

impl ArtifactStore {
    /// Creates an adapter whose key prefix is derived from, but never exposes, `tenant_id`.
    pub fn new(client: S3Client, tenant_id: &str) -> Self {
        Self {
            client,
            // Tenant identifiers are never placed in keys directly. A fixed-width
            // derivation prevents delimiter injection and cross-tenant prefixes.
            tenant_prefix: format!("tenants/{}/", hex_sha256(tenant_id.as_bytes())),
        }
    }

    /// Returns the fixed-width S3 prefix assigned to this tenant.
    pub fn tenant_prefix(&self) -> &str {
        &self.tenant_prefix
    }

    /// Creates the payload first and its manifest publication marker last.
    pub async fn publish_manifest_last(
        &self,
        blob_digest: &ContentDigest,
        blob: Bytes,
        manifest_digest: &ContentDigest,
        manifest: Bytes,
    ) -> Result<Publication, ArtifactError> {
        verify_bytes(blob_digest, &blob)?;
        verify_bytes(manifest_digest, &manifest)?;

        let blob_created = self
            .put_immutable(
                self.blob_key(blob_digest)?,
                blob,
                "application/octet-stream",
            )
            .await?;
        // The manifest is the publication marker and is deliberately written only
        // after every immutable payload it references is durable.
        let manifest_created = self
            .put_immutable(
                self.manifest_key(manifest_digest)?,
                manifest,
                "application/json",
            )
            .await?;
        Ok(Publication {
            blob_created,
            manifest_created,
        })
    }

    /// Tests payload existence using HEAD without downloading any object bytes.
    pub async fn exists(&self, digest: &ContentDigest) -> Result<bool, ArtifactError> {
        match self
            .client
            .head_object(HeadObjectRequest::new(self.blob_key(digest)?))
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.category() == ErrorCategory::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Streams a payload into `writer` and verifies its content-address digest.
    ///
    /// Callers publishing the local result should write to a temporary destination
    /// and rename it only after this method returns successfully.
    pub async fn download_verified<W>(
        &self,
        digest: &ContentDigest,
        writer: &mut W,
    ) -> Result<u64, ArtifactError>
    where
        W: AsyncWrite + Unpin,
    {
        let mut body = self
            .client
            .get_object(GetObjectRequest::new(self.blob_key(digest)?))
            .await?
            .body;
        let mut hasher = Sha256::new();
        let mut written = 0_u64;
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            writer.write_all(&chunk).await?;
            hasher.update(&chunk);
            written =
                written
                    .checked_add(u64::try_from(chunk.len()).map_err(|_| {
                        S3Error::integrity("download chunk length does not fit in u64")
                    })?)
                    .ok_or_else(|| S3Error::integrity("download length overflow"))?;
        }
        writer.flush().await?;
        if encode_hex(&hasher.finalize()) != digest.as_str() {
            return Err(ArtifactError::DigestMismatch);
        }
        Ok(written)
    }

    /// Deletes one content-addressed payload.
    pub async fn delete(&self, digest: &ContentDigest) -> Result<(), ArtifactError> {
        self.client
            .delete_object(DeleteObjectRequest::new(self.blob_key(digest)?))
            .await?;
        Ok(())
    }

    /// Lists payloads and deletes those absent from the caller's live digest set.
    pub async fn collect_unreferenced(
        &self,
        live: &BTreeSet<ContentDigest>,
    ) -> Result<usize, ArtifactError> {
        let prefix = format!("{}blobs/sha256/", self.tenant_prefix);
        let pages = self
            .client
            .list_objects_v2_all(
                ListObjectsV2Request {
                    prefix: Some(prefix.clone()),
                    ..ListObjectsV2Request::default()
                },
                LISTING_PAGE_LIMIT,
            )
            .await?;
        let mut unreachable = Vec::new();
        for object in pages.into_iter().flat_map(|page| page.objects) {
            let Some(suffix) = object.key.as_str().strip_prefix(&prefix) else {
                continue;
            };
            let Ok(digest) = ContentDigest::new(suffix) else {
                continue;
            };
            if !live.contains(&digest) {
                unreachable.push(DeleteObjectIdentifier::new(object.key));
            }
        }

        let mut deleted = 0_usize;
        for batch in unreachable.chunks(DELETE_BATCH_LIMIT) {
            let request = DeleteObjectsRequest::new(batch.to_vec())
                .map_err(|error| S3Error::configuration(error.to_string()))?;
            let output = self.client.delete_objects(request).await?;
            if !output.errors.is_empty() {
                return Err(S3Error::invalid_response(
                    "S3 reported one or more per-object deletion failures",
                )
                .into());
            }
            deleted = deleted
                .checked_add(output.deleted.len())
                .ok_or_else(|| S3Error::integrity("deleted object count overflow"))?;
        }
        Ok(deleted)
    }

    /// Uploads a verified file with managed multipart cancellation and a deadline.
    ///
    /// Cancelling the token or reaching `deadline` drops the managed upload
    /// future; the client then owns abort cleanup for any created upload. The
    /// existence check avoids replacing an already-published digest. Callers
    /// must serialize concurrent publication of the same digest because the
    /// managed multipart completion API has no destination precondition.
    pub async fn upload_file_until(
        &self,
        digest: &ContentDigest,
        path: impl AsRef<Path>,
        deadline: Duration,
        cancellation: &CancellationToken,
    ) -> Result<(), ArtifactError> {
        if self.exists(digest).await? {
            return Ok(());
        }
        let snapshot = snapshot_verified(path.as_ref(), digest).await?;
        let options = MultipartOptions::default().with_transfer_timeout(deadline)?;
        let request =
            ManagedMultipartUploadRequest::from_path(self.blob_key(digest)?, snapshot.path())
                .with_options(options);
        let upload = self.client.multipart_upload(request);
        tokio::pin!(upload);

        tokio::select! {
            result = &mut upload => {
                match result {
                    Err(error) if error.timeout_phase() == Some(TimeoutPhase::Operation) => {
                        Err(ArtifactError::DeadlineExceeded)
                    }
                    result => {
                        result?;
                        Ok(())
                    }
                }
            }
            () = cancellation.cancelled() => Err(ArtifactError::Cancelled),
        }
        // Dropping `upload` on either early-return branch triggers the client's
        // owned cancellation path, which aborts an already-created multipart upload.
    }

    /// Aborts tenant uploads whose reported initiation time is at or before a cutoff.
    pub async fn abort_stale_multipart(
        &self,
        initiated_before: OffsetDateTime,
        maximum_pages: usize,
    ) -> Result<usize, ArtifactError> {
        if maximum_pages == 0 {
            return Err(ArtifactError::ListingPageLimit);
        }
        let prefix = format!("{}blobs/sha256/", self.tenant_prefix);
        let mut request = ListMultipartUploadsRequest {
            prefix: Some(prefix),
            ..ListMultipartUploadsRequest::default()
        };
        let mut aborted = 0_usize;
        for _ in 0..maximum_pages {
            let page = self.client.list_multipart_uploads(request.clone()).await?;
            for upload in page.uploads.iter().filter(|upload| {
                upload
                    .initiated
                    .is_some_and(|time| time <= initiated_before)
            }) {
                self.client
                    .abort_multipart_upload(AbortMultipartUploadRequest::new(
                        upload.key.clone(),
                        upload.upload_id().clone(),
                    ))
                    .await?;
                aborted = aborted
                    .checked_add(1)
                    .ok_or_else(|| S3Error::integrity("aborted upload count overflow"))?;
            }
            if !page.is_truncated {
                return Ok(aborted);
            }
            request.key_marker = page.next_key_marker;
            request.upload_id_marker = page.next_upload_id_marker;
            if request.key_marker.is_none() || request.upload_id_marker.is_none() {
                return Err(S3Error::invalid_response(
                    "truncated multipart listing omitted a required next marker",
                )
                .into());
            }
        }
        Err(ArtifactError::ListingPageLimit)
    }

    async fn put_immutable(
        &self,
        key: ObjectKey,
        body: Bytes,
        content_type: &str,
    ) -> Result<bool, ArtifactError> {
        let mut request = PutObjectRequest::new(key, ByteStream::from_bytes(body));
        request.content_type = Some(content_type.to_owned());
        request.conditions.if_none_match = Some("*".to_owned());
        match self.client.put_object(request).await {
            Ok(_) => Ok(true),
            Err(error) if error.category() == ErrorCategory::Precondition => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn blob_key(&self, digest: &ContentDigest) -> Result<ObjectKey, ObjectKeyError> {
        ObjectKey::new(format!(
            "{}blobs/sha256/{}",
            self.tenant_prefix,
            digest.as_str()
        ))
    }

    fn manifest_key(&self, digest: &ContentDigest) -> Result<ObjectKey, ObjectKeyError> {
        ObjectKey::new(format!(
            "{}manifests/{}.json",
            self.tenant_prefix,
            digest.as_str()
        ))
    }
}

/// Builds a client from injected credentials and an optional custom endpoint.
pub fn client_from_environment() -> Result<S3Client, ArtifactError> {
    let region = env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
    let endpoint = env::var("S3_ENDPOINT")
        .map_or_else(|_| Endpoint::for_aws_region(&region), Endpoint::new)?;
    let credentials = Credentials::new(
        required_env("S3_ACCESS_KEY_ID")?,
        required_env("S3_SECRET_ACCESS_KEY")?,
        env::var("S3_SESSION_TOKEN").ok(),
    )?;
    let provider: Arc<dyn CredentialsProvider> =
        Arc::new(StaticCredentialsProvider::new(credentials));
    let mut builder = S3Config::builder()
        .endpoint(endpoint.clone())
        .region(region)
        .bucket(required_env("S3_BUCKET")?)
        .addressing_style(AddressingStyle::Path)
        .credentials_provider(provider);
    if !endpoint.is_https() {
        builder = builder.allow_http_for_local_testing();
    }
    Ok(S3Client::new(builder.build()?)?)
}

fn required_env(name: &'static str) -> Result<String, ArtifactError> {
    env::var(name).map_err(|_| ArtifactError::MissingEnvironment(name))
}

fn verify_bytes(expected: &ContentDigest, bytes: &[u8]) -> Result<(), ArtifactError> {
    if ContentDigest::of(bytes) == *expected {
        Ok(())
    } else {
        Err(ArtifactError::DigestMismatch)
    }
}

async fn snapshot_verified(
    path: &Path,
    expected: &ContentDigest,
) -> Result<tempfile::NamedTempFile, ArtifactError> {
    let mut source = File::open(path).await?;
    let snapshot = tempfile::NamedTempFile::new()?;
    let mut destination = File::from_std(snapshot.reopen()?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = source.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        destination.write_all(&buffer[..read]).await?;
    }
    destination.flush().await?;
    if encode_hex(&hasher.finalize()) != expected.as_str() {
        return Err(ArtifactError::DigestMismatch);
    }
    Ok(snapshot)
}

fn hex_sha256(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[tokio::main]
async fn main() -> Result<(), ArtifactError> {
    let store = ArtifactStore::new(client_from_environment()?, &required_env("TENANT_ID")?);
    println!("artifact prefix: {}", store.tenant_prefix());
    Ok(())
}
