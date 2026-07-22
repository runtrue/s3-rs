//! Bounded orchestration for managed multipart uploads.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::S3Client;
use super::request::OperationDeadline;
use crate::error::S3Error;
use crate::operation::{
    AbortMultipartUploadRequest, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest,
    CompletedPart, CreateMultipartUploadRequest, ManagedMultipartUploadRequest, MultipartUpload,
    MultipartUploadSource, ObjectKey, PartNumber, UploadId, UploadPartRequest,
};
use crate::stream::{ByteStream, FileSnapshot};

impl S3Client {
    /// Uploads a replayable bytes or file source using bounded multipart requests.
    ///
    /// Part size, concurrency, total in-flight part bytes, and deadlines are
    /// derived from the request's [`crate::MultipartOptions`]. File input is
    /// snapshotted before an upload is created. Dropping this future cancels
    /// outstanding parts and leaves an owned cleanup task to quiesce transmitted
    /// requests before aborting the upload. A normal failure waits for cleanup;
    /// if cleanup cannot be confirmed,
    /// [`S3Error::cleanup_failure`] exposes that error while preserving the
    /// original failure.
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
        let transfer_deadline = OperationDeadline::new(request.options().transfer_timeout());
        let cleanup_timeout = request.options().cleanup_timeout();
        let cancellation = Cancellation::new();
        let worker_cancellation = cancellation.clone();
        let client = self.clone();
        let handle = runtime.spawn(async move {
            run_multipart_upload(
                client,
                request,
                worker_cancellation,
                transfer_deadline,
                cleanup_timeout,
            )
            .await
        });
        OwnedMultipartTask {
            cancellation,
            handle,
        }
        .await
    }
}

struct OwnedMultipartTask {
    cancellation: Cancellation,
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

#[derive(Clone)]
struct Cancellation {
    sender: watch::Sender<bool>,
}

impl Cancellation {
    fn new() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }

    fn cancel(&self) {
        self.sender.send_replace(true);
    }

    fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

async fn run_multipart_upload(
    client: S3Client,
    request: ManagedMultipartUploadRequest,
    cancellation: Cancellation,
    transfer_deadline: OperationDeadline,
    cleanup_timeout: Duration,
) -> Result<CompleteMultipartUploadOutput, S3Error> {
    let options = request.options();
    let source = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(cancelled()),
        result = tokio::time::timeout_at(
            transfer_deadline.instant(),
            PreparedMultipartSource::prepare(request.source),
        ) => result.map_err(|_| transfer_timeout())??,
    };
    let part_size = options.part_size();
    let part_count = source.part_count(part_size)?;
    if cancellation.is_cancelled() {
        return Err(cancelled());
    }

    let mut create = CreateMultipartUploadRequest::new(request.key.clone());
    create.content_type = request.content_type;
    create.user_metadata = request.user_metadata;
    create.checksum_algorithm = request.checksum_algorithm;
    // Once creation starts, allow it to finish even after cancellation so a
    // successful response cannot be discarded together with the upload ID
    // needed for cleanup.
    let created = client
        .create_multipart_upload_with_deadline(create, &transfer_deadline)
        .await?;
    let upload_id = created.upload_id().clone();

    let result = upload_parts(
        PartUploadContext {
            client: client.clone(),
            key: request.key.clone(),
            upload_id: upload_id.clone(),
            source: Arc::new(source),
            part_size,
            checksum_algorithm: request.checksum_algorithm,
            deadline: transfer_deadline,
        },
        part_count,
        options.concurrency(),
        &cancellation,
        cleanup_timeout,
    )
    .await;

    let upload = match result {
        Ok(upload) => upload,
        Err(failure) => {
            return fail_with_abort(&client, request.key, upload_id, failure).await;
        }
    };
    let completion = match CompleteMultipartUploadRequest::new(
        upload.key().clone(),
        upload.upload_id().clone(),
        upload.completed_parts().to_vec(),
    ) {
        Ok(completion) => completion,
        Err(error) => {
            return fail_with_abort(
                &client,
                request.key,
                upload_id,
                ManagedUploadFailure::new(S3Error::integrity(error.to_string()), cleanup_timeout),
            )
            .await;
        }
    };

    let completion = tokio::select! {
        biased;
        result = client.complete_multipart_upload_with_deadline(
            completion,
            &transfer_deadline,
        ) => result,
        () = cancellation.cancelled() => Err(cancelled()),
    };

    match completion {
        Ok(output) => Ok(output),
        Err(primary) => {
            fail_with_abort(
                &client,
                request.key,
                upload_id,
                ManagedUploadFailure::new(primary, cleanup_timeout),
            )
            .await
        }
    }
}

#[derive(Clone)]
struct PartUploadContext {
    client: S3Client,
    key: ObjectKey,
    upload_id: UploadId,
    source: Arc<PreparedMultipartSource>,
    part_size: u64,
    checksum_algorithm: Option<crate::operation::ChecksumAlgorithm>,
    deadline: OperationDeadline,
}

async fn upload_parts(
    context: PartUploadContext,
    part_count: u16,
    concurrency: usize,
    cancellation: &Cancellation,
    cleanup_timeout: Duration,
) -> Result<MultipartUpload, ManagedUploadFailure> {
    let mut next_part = 1_u16;
    let mut pending = FuturesUnordered::new();
    let mut upload = MultipartUpload::new(context.key.clone(), context.upload_id.clone());
    let deadline_sleep = tokio::time::sleep_until(context.deadline.instant());
    tokio::pin!(deadline_sleep);

    loop {
        while pending.len() < concurrency && next_part <= part_count {
            pending.push(upload_one_part(context.clone(), next_part));
            next_part += 1;
        }
        if pending.is_empty() {
            return Ok(upload);
        }
        let completed = tokio::select! {
            () = cancellation.cancelled() => Err(cancelled()),
            () = &mut deadline_sleep => Err(transfer_timeout()),
            result = pending.next() => result
                .ok_or_else(|| S3Error::integrity("multipart scheduler lost an in-flight part"))
                .and_then(|result| result),
        };
        let completed = match completed {
            Ok(completed) => completed,
            Err(primary) => {
                return Err(quiesce_part_requests(primary, &mut pending, cleanup_timeout).await);
            }
        };
        if let Err(error) = upload.record_part(completed) {
            return Err(quiesce_part_requests(
                S3Error::integrity(error.to_string()),
                &mut pending,
                cleanup_timeout,
            )
            .await);
        }
    }
}

struct ManagedUploadFailure {
    primary: S3Error,
    cleanup_deadline: OperationDeadline,
    quiesce_failure: Option<S3Error>,
}

impl ManagedUploadFailure {
    fn new(primary: S3Error, cleanup_timeout: Duration) -> Self {
        Self {
            primary,
            cleanup_deadline: OperationDeadline::new(cleanup_timeout),
            quiesce_failure: None,
        }
    }
}

async fn quiesce_part_requests<F>(
    primary: S3Error,
    pending: &mut FuturesUnordered<F>,
    cleanup_timeout: Duration,
) -> ManagedUploadFailure
where
    F: Future<Output = Result<CompletedPart, S3Error>>,
{
    let mut failure = ManagedUploadFailure::new(primary, cleanup_timeout);
    let settle_until = tokio::time::Instant::now() + cleanup_timeout / 2;
    while !pending.is_empty() {
        match tokio::time::timeout_at(settle_until, pending.next()).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                failure.quiesce_failure = Some(S3Error::timeout(
                    crate::error::TimeoutPhase::Operation,
                    "multipart cleanup could not quiesce in-flight part requests",
                ));
                break;
            }
        }
    }
    failure
}

async fn fail_with_abort(
    client: &S3Client,
    key: ObjectKey,
    upload_id: UploadId,
    failure: ManagedUploadFailure,
) -> Result<CompleteMultipartUploadOutput, S3Error> {
    let abort = client
        .abort_multipart_upload_with_deadline(
            AbortMultipartUploadRequest::new(key, upload_id),
            &failure.cleanup_deadline,
        )
        .await
        .map(|_| ());
    let cleanup = match (abort, failure.quiesce_failure) {
        (Ok(()), None) => Ok(()),
        (Err(error), None) | (Ok(()), Some(error)) => Err(error),
        (Err(abort), Some(quiesce)) => Err(abort.with_cleanup_failure(quiesce)),
    };
    Err(attach_cleanup_failure(failure.primary, cleanup))
}

async fn upload_one_part(
    context: PartUploadContext,
    part_number: u16,
) -> Result<CompletedPart, S3Error> {
    let body = context
        .source
        .read_part(context.part_size, part_number)
        .await?;
    let number = PartNumber::new(part_number)
        .ok_or_else(|| S3Error::integrity("generated multipart part number is invalid"))?;
    let checksum = match context.checksum_algorithm {
        Some(algorithm) => tokio::time::timeout_at(
            context.deadline.instant(),
            crate::operation::Checksum::calculate_cooperatively(algorithm, &body),
        )
        .await
        .map_err(|_| transfer_timeout())?
        .map_err(|error| S3Error::unsupported(error.to_string()))?,
        None => crate::operation::Checksum::default(),
    };
    let output = context
        .client
        .upload_part_with_deadline(
            UploadPartRequest::new(
                context.key,
                context.upload_id,
                number,
                ByteStream::from_bytes(body),
            )
            .with_checksum(checksum),
            &context.deadline,
        )
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

fn transfer_timeout() -> S3Error {
    S3Error::timeout(
        crate::error::TimeoutPhase::Operation,
        "managed multipart upload exceeded its transfer deadline",
    )
}

enum PreparedMultipartSource {
    Bytes(Bytes),
    File(FileSnapshot),
}

impl PreparedMultipartSource {
    async fn prepare(source: MultipartUploadSource) -> Result<Self, S3Error> {
        match source {
            MultipartUploadSource::Bytes(bytes) => Ok(Self::Bytes(bytes)),
            MultipartUploadSource::File(path) => Self::snapshot(path).await,
        }
    }

    async fn snapshot(path: PathBuf) -> Result<Self, S3Error> {
        FileSnapshot::create(path, false).await.map(Self::File)
    }

    fn length(&self) -> Result<u64, S3Error> {
        match self {
            Self::Bytes(bytes) => u64::try_from(bytes.len())
                .map_err(|_| S3Error::configuration("multipart byte length does not fit in u64")),
            Self::File(snapshot) => Ok(snapshot.length()),
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
            Self::File(snapshot) => snapshot.read_range(offset, length_usize).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCategory;

    #[tokio::test]
    async fn task_drop_signals_owned_cleanup_worker() {
        let cancellation = Cancellation::new();
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

    #[tokio::test]
    async fn cleanup_reports_part_requests_that_cannot_quiesce() {
        let mut pending = FuturesUnordered::new();
        pending.push(std::future::pending::<Result<CompletedPart, S3Error>>());

        let failure = quiesce_part_requests(
            S3Error::cancellation("primary"),
            &mut pending,
            Duration::from_millis(10),
        )
        .await;

        assert!(failure.quiesce_failure.is_some());
        assert_eq!(failure.primary.category(), ErrorCategory::Cancellation);
    }

    #[test]
    fn part_count_rejects_empty_and_excessive_sources() {
        let empty = PreparedMultipartSource::Bytes(Bytes::new());
        assert!(empty.part_count(5).is_err());
        let too_large = PreparedMultipartSource::Bytes(Bytes::from(vec![0_u8; 10_001]));
        assert!(too_large.part_count(1).is_err());
    }
}
