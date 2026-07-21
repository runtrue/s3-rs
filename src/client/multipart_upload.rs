//! Bounded orchestration for managed multipart uploads.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use tokio::fs::File;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::S3Client;
use crate::error::S3Error;
use crate::operation::{
    AbortMultipartUploadRequest, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest,
    CompletedPart, CreateMultipartUploadRequest, ManagedMultipartUploadRequest, MultipartUpload,
    MultipartUploadSource, ObjectKey, PartNumber, UploadId, UploadPartRequest,
};
use crate::stream::ByteStream;

impl S3Client {
    /// Uploads a replayable bytes or file source using bounded multipart requests.
    ///
    /// Part size, concurrency, and total in-flight part bytes are taken from the
    /// client configuration. File input is snapshotted before an upload is
    /// created. Dropping this future cancels outstanding parts and leaves an
    /// owned cleanup task to abort the upload. A normal failure waits for abort;
    /// if abort also fails, [`S3Error::cleanup_failure`] exposes that error while
    /// preserving the original failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot be prepared, exceeds S3's 10,000
    /// part limit, a part fails, cancellation occurs, completion fails, or abort
    /// cleanup fails after another error.
    pub async fn multipart_upload(
        &self,
        request: ManagedMultipartUploadRequest,
    ) -> Result<CompleteMultipartUploadOutput, S3Error> {
        let runtime = tokio::runtime::Handle::try_current().map_err(S3Error::transport)?;
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let client = self.clone();
        let handle = runtime
            .spawn(async move { run_multipart_upload(client, request, worker_cancellation).await });
        OwnedMultipartTask {
            cancellation,
            handle,
        }
        .await
    }
}

struct OwnedMultipartTask {
    cancellation: CancellationToken,
    handle: JoinHandle<Result<CompleteMultipartUploadOutput, S3Error>>,
}

impl Future for OwnedMultipartTask {
    type Output = Result<CompleteMultipartUploadOutput, S3Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.handle).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(error)) => Poll::Ready(Err(S3Error::transport(error))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for OwnedMultipartTask {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn run_multipart_upload(
    client: S3Client,
    request: ManagedMultipartUploadRequest,
    cancellation: CancellationToken,
) -> Result<CompleteMultipartUploadOutput, S3Error> {
    let source = tokio::select! {
        () = cancellation.cancelled() => return Err(cancelled()),
        result = PreparedMultipartSource::prepare(request.source) => result?,
    };
    let part_size = client.config().multipart_part_size();
    let part_count = source.part_count(part_size)?;

    let mut create = CreateMultipartUploadRequest::new(request.key.clone());
    create.content_type = request.content_type;
    create.user_metadata = request.user_metadata;
    // Once creation starts, allow it to finish even after cancellation so a
    // successful response cannot be discarded together with the upload ID
    // needed for cleanup.
    let created = client.create_multipart_upload(create).await?;
    let upload_id = created.upload_id().clone();

    let result = upload_parts(
        &client,
        request.key.clone(),
        upload_id.clone(),
        Arc::new(source),
        part_size,
        part_count,
        &cancellation,
    )
    .await
    .and_then(|upload| {
        CompleteMultipartUploadRequest::new(
            upload.key().clone(),
            upload.upload_id().clone(),
            upload.completed_parts().to_vec(),
        )
        .map_err(|error| S3Error::integrity(error.to_string()))
    });

    let completion = match result {
        Ok(completion) => {
            tokio::select! {
                biased;
                result = client.complete_multipart_upload(completion) => result,
                () = cancellation.cancelled() => Err(cancelled()),
            }
        }
        Err(error) => Err(error),
    };

    match completion {
        Ok(output) => Ok(output),
        Err(primary) => {
            let cleanup = client
                .abort_multipart_upload(AbortMultipartUploadRequest::new(request.key, upload_id))
                .await
                .map(|_| ());
            Err(attach_cleanup_failure(primary, cleanup))
        }
    }
}

async fn upload_parts(
    client: &S3Client,
    key: ObjectKey,
    upload_id: UploadId,
    source: Arc<PreparedMultipartSource>,
    part_size: u64,
    part_count: u16,
    cancellation: &CancellationToken,
) -> Result<MultipartUpload, S3Error> {
    let concurrency = effective_concurrency(
        client.config().multipart_concurrency(),
        client.config().max_multipart_in_flight_bytes(),
        part_size,
    )?;
    let mut next_part = 1_u16;
    let mut pending = FuturesUnordered::new();
    let mut upload = MultipartUpload::new(key.clone(), upload_id.clone());

    loop {
        while pending.len() < concurrency && next_part <= part_count {
            pending.push(upload_one_part(
                client.clone(),
                key.clone(),
                upload_id.clone(),
                Arc::clone(&source),
                part_size,
                next_part,
            ));
            next_part += 1;
        }
        if pending.is_empty() {
            return Ok(upload);
        }
        let completed = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled()),
            result = pending.next() => result
                .ok_or_else(|| S3Error::integrity("multipart scheduler lost an in-flight part"))??,
        };
        upload
            .record_part(completed)
            .map_err(|error| S3Error::integrity(error.to_string()))?;
    }
}

async fn upload_one_part(
    client: S3Client,
    key: ObjectKey,
    upload_id: UploadId,
    source: Arc<PreparedMultipartSource>,
    part_size: u64,
    part_number: u16,
) -> Result<CompletedPart, S3Error> {
    let body = source.read_part(part_size, part_number).await?;
    let number = PartNumber::new(part_number)
        .ok_or_else(|| S3Error::integrity("generated multipart part number is invalid"))?;
    let output = client
        .upload_part(UploadPartRequest::new(
            key,
            upload_id,
            number,
            ByteStream::from_bytes(body),
        ))
        .await?;
    CompletedPart::new(output.part_number.get(), output.e_tag)
        .map(|part| part.with_checksum(output.checksum))
        .map_err(|error| S3Error::invalid_response(error.to_string()))
}

fn attach_cleanup_failure(primary: S3Error, cleanup: Result<(), S3Error>) -> S3Error {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => primary.with_cleanup_failure(cleanup),
    }
}

fn cancelled() -> S3Error {
    S3Error::cancellation("multipart upload was cancelled")
}

fn effective_concurrency(
    configured: usize,
    byte_budget: u64,
    part_size: u64,
) -> Result<usize, S3Error> {
    let byte_slots = byte_budget / part_size;
    let byte_slots = usize::try_from(byte_slots).unwrap_or(usize::MAX);
    let concurrency = configured.min(byte_slots);
    if concurrency == 0 {
        return Err(S3Error::configuration(
            "multipart in-flight byte budget cannot hold one part",
        ));
    }
    Ok(concurrency)
}

enum PreparedMultipartSource {
    Bytes(Bytes),
    File {
        path: tempfile::TempPath,
        length: u64,
    },
}

impl PreparedMultipartSource {
    async fn prepare(source: MultipartUploadSource) -> Result<Self, S3Error> {
        match source {
            MultipartUploadSource::Bytes(bytes) => Ok(Self::Bytes(bytes)),
            MultipartUploadSource::File(path) => Self::snapshot(path).await,
        }
    }

    async fn snapshot(path: PathBuf) -> Result<Self, S3Error> {
        let mut input = File::open(path).await.map_err(S3Error::transport)?;
        let metadata = input.metadata().await.map_err(S3Error::transport)?;
        if !metadata.is_file() {
            return Err(S3Error::configuration(
                "multipart upload path must identify a regular file",
            ));
        }
        let snapshot = tempfile::NamedTempFile::new().map_err(S3Error::transport)?;
        let (snapshot_file, snapshot_path) = snapshot.into_parts();
        let mut output = File::from_std(snapshot_file);
        let length = tokio::io::copy(&mut input, &mut output)
            .await
            .map_err(S3Error::transport)?;
        output.flush().await.map_err(S3Error::transport)?;
        drop(output);
        let mut permissions = std::fs::metadata(&snapshot_path)
            .map_err(S3Error::transport)?
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&snapshot_path, permissions).map_err(S3Error::transport)?;
        let actual = tokio::fs::metadata(&snapshot_path)
            .await
            .map_err(S3Error::transport)?
            .len();
        if actual != length {
            return Err(S3Error::integrity(
                "multipart source snapshot length changed while preparing",
            ));
        }
        Ok(Self::File {
            path: snapshot_path,
            length,
        })
    }

    fn length(&self) -> Result<u64, S3Error> {
        match self {
            Self::Bytes(bytes) => u64::try_from(bytes.len())
                .map_err(|_| S3Error::configuration("multipart byte length does not fit in u64")),
            Self::File { length, .. } => Ok(*length),
        }
    }

    fn part_count(&self, part_size: u64) -> Result<u16, S3Error> {
        let length = self.length()?;
        if length == 0 {
            return Err(S3Error::configuration(
                "managed multipart uploads require a non-empty source",
            ));
        }
        let count = length.div_ceil(part_size);
        if count > u64::from(PartNumber::MAX) {
            return Err(S3Error::configuration(
                "multipart source exceeds the configured 10,000-part limit",
            ));
        }
        u16::try_from(count)
            .map_err(|_| S3Error::configuration("multipart part count does not fit in u16"))
    }

    async fn read_part(&self, part_size: u64, part_number: u16) -> Result<Bytes, S3Error> {
        let offset = u64::from(part_number - 1)
            .checked_mul(part_size)
            .ok_or_else(|| S3Error::integrity("multipart part offset overflow"))?;
        let remaining = self
            .length()?
            .checked_sub(offset)
            .ok_or_else(|| S3Error::integrity("multipart part offset exceeds source length"))?;
        let length = remaining.min(part_size);
        let length_usize = usize::try_from(length)
            .map_err(|_| S3Error::configuration("multipart part size does not fit in usize"))?;
        match self {
            Self::Bytes(bytes) => {
                let start = usize::try_from(offset).map_err(|_| {
                    S3Error::configuration("multipart byte offset does not fit in usize")
                })?;
                Ok(bytes.slice(start..start + length_usize))
            }
            Self::File { path, .. } => {
                use tokio::io::AsyncSeekExt as _;

                let mut file = File::open(path).await.map_err(S3Error::transport)?;
                file.seek(io::SeekFrom::Start(offset))
                    .await
                    .map_err(S3Error::transport)?;
                let mut bytes = BytesMut::zeroed(length_usize);
                file.read_exact(&mut bytes)
                    .await
                    .map_err(S3Error::transport)?;
                Ok(bytes.freeze())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCategory;

    #[tokio::test]
    async fn task_drop_signals_owned_cleanup_worker() {
        let cancellation = CancellationToken::new();
        let worker = cancellation.clone();
        let (finished, observed) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            worker.cancelled().await;
            let _ = finished.send(());
            Err(cancelled())
        });
        drop(OwnedMultipartTask {
            cancellation,
            handle,
        });
        observed.await.unwrap();
    }

    #[test]
    fn cleanup_failure_does_not_replace_primary_error() {
        let primary = S3Error::integrity("primary");
        let cleanup = S3Error::invalid_response("cleanup");
        let combined = attach_cleanup_failure(primary, Err(cleanup));
        assert_eq!(combined.category(), ErrorCategory::Integrity);
        assert_eq!(combined.message(), "primary");
        assert_eq!(
            combined.cleanup_failure().map(S3Error::category),
            Some(ErrorCategory::InvalidResponse)
        );
    }

    #[tokio::test]
    async fn byte_parts_are_exact_and_bounded() {
        let source = PreparedMultipartSource::Bytes(Bytes::from_static(b"abcdefghij"));
        assert_eq!(source.part_count(4).unwrap(), 3);
        assert_eq!(source.read_part(4, 1).await.unwrap(), "abcd");
        assert_eq!(source.read_part(4, 2).await.unwrap(), "efgh");
        assert_eq!(source.read_part(4, 3).await.unwrap(), "ij");
    }

    #[tokio::test]
    async fn file_source_is_snapshotted_before_parts_are_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source");
        tokio::fs::write(&path, b"original").await.unwrap();
        let source = PreparedMultipartSource::snapshot(path.clone())
            .await
            .unwrap();
        tokio::fs::write(path, b"changed!").await.unwrap();
        assert_eq!(source.read_part(8, 1).await.unwrap(), "original");
    }

    #[test]
    fn part_count_rejects_empty_and_excessive_sources() {
        let empty = PreparedMultipartSource::Bytes(Bytes::new());
        assert!(empty.part_count(5).is_err());
        let too_large = PreparedMultipartSource::File {
            path: tempfile::NamedTempFile::new().unwrap().into_temp_path(),
            length: 10_001,
        };
        assert!(too_large.part_count(1).is_err());
    }

    #[test]
    fn concurrency_is_limited_by_count_and_byte_budget() {
        assert_eq!(effective_concurrency(8, 24, 8).unwrap(), 3);
        assert_eq!(effective_concurrency(2, 24, 8).unwrap(), 2);
        assert!(effective_concurrency(2, 7, 8).is_err());
    }
}
