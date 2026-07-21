//! Replay-aware upload bodies and bounded download streams.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use futures_core::Stream;
use futures_util::StreamExt;
use hyper::body::{Body, Frame, SizeHint};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::{Instant, Sleep};
use tokio_util::io::ReaderStream;

use crate::error::S3Error;

type UploadItems = Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + 'static>>;
type DownloadItems = Pin<Box<dyn Stream<Item = Result<Bytes, S3Error>> + Send + 'static>>;

/// An upload body with explicit length, integrity, and replay semantics.
///
/// Byte and file bodies are replayable. A caller-provided stream is one-shot
/// and requires both its exact content length and SHA-256 digest so the client
/// never buffers it or silently switches to unsigned payloads.
pub struct ByteStream {
    source: UploadSource,
}

enum UploadSource {
    Bytes(Bytes),
    File(PathBuf),
    Stream {
        stream: UploadItems,
        length: u64,
        sha256: [u8; 32],
    },
}

impl ByteStream {
    /// Creates a replayable in-memory body.
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        Self {
            source: UploadSource::Bytes(bytes.into()),
        }
    }

    /// Creates a replayable file body.
    ///
    /// The explicitly supplied file is copied into a private disk-backed snapshot
    /// while it is hashed. Every retry reads that immutable snapshot, so changes
    /// to the original path cannot alter the signed upload bytes.
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            source: UploadSource::File(path.as_ref().to_owned()),
        }
    }

    /// Creates a one-shot stream with a caller-supplied exact digest.
    ///
    /// The stream is never retried. `sha256` is the digest of exactly `length`
    /// bytes and is used for `SigV4` payload signing.
    pub fn from_stream<S>(stream: S, length: u64, sha256: [u8; 32]) -> Self
    where
        S: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
    {
        Self {
            source: UploadSource::Stream {
                stream: Box::pin(stream),
                length,
                sha256,
            },
        }
    }

    pub(crate) async fn prepare(self) -> Result<PreparedBody, S3Error> {
        match self.source {
            UploadSource::Bytes(bytes) => {
                let sha256 = Sha256::digest(&bytes).into();
                let length = u64::try_from(bytes.len()).map_err(|_| {
                    S3Error::configuration("in-memory body length does not fit in u64")
                })?;
                Ok(PreparedBody {
                    source: PreparedSource::Bytes(bytes),
                    length,
                    sha256,
                })
            }
            UploadSource::File(path) => prepare_file(path).await,
            UploadSource::Stream {
                stream,
                length,
                sha256,
            } => Ok(PreparedBody {
                source: PreparedSource::OneShot(Mutex::new(Some(stream))),
                length,
                sha256,
            }),
        }
    }
}

impl From<Bytes> for ByteStream {
    fn from(value: Bytes) -> Self {
        Self::from_bytes(value)
    }
}

impl From<Vec<u8>> for ByteStream {
    fn from(value: Vec<u8>) -> Self {
        Self::from_bytes(value)
    }
}

impl From<&'static [u8]> for ByteStream {
    fn from(value: &'static [u8]) -> Self {
        Self::from_bytes(Bytes::from_static(value))
    }
}

impl fmt::Debug for ByteStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, replayable) = match self.source {
            UploadSource::Bytes(_) => ("bytes", true),
            UploadSource::File(_) => ("file", true),
            UploadSource::Stream { .. } => ("stream", false),
        };
        formatter
            .debug_struct("ByteStream")
            .field("kind", &kind)
            .field("replayable", &replayable)
            .finish_non_exhaustive()
    }
}

async fn prepare_file(path: PathBuf) -> Result<PreparedBody, S3Error> {
    let mut file = File::open(&path).await.map_err(S3Error::transport)?;
    let metadata = file.metadata().await.map_err(S3Error::transport)?;
    if !metadata.is_file() {
        return Err(S3Error::configuration(
            "upload path must identify a regular file",
        ));
    }

    // Copy to a private disk-backed snapshot while hashing. Every attempt opens
    // this immutable snapshot, so the signed bytes cannot diverge if the caller's
    // original path is replaced or modified during retries.
    let snapshot = tempfile::NamedTempFile::new().map_err(S3Error::transport)?;
    let (snapshot_file, snapshot_path) = snapshot.into_parts();
    let mut snapshot_file = File::from_std(snapshot_file);
    let mut hasher = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(S3Error::transport)?;
        if read == 0 {
            break;
        }
        length =
            length
                .checked_add(u64::try_from(read).map_err(|_| {
                    S3Error::integrity("upload file chunk length does not fit in u64")
                })?)
                .ok_or_else(|| S3Error::integrity("upload file length overflow"))?;
        hasher.update(&buffer[..read]);
        snapshot_file
            .write_all(&buffer[..read])
            .await
            .map_err(S3Error::transport)?;
    }
    snapshot_file.flush().await.map_err(S3Error::transport)?;
    drop(snapshot_file);
    let mut permissions = std::fs::metadata(&snapshot_path)
        .map_err(S3Error::transport)?
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&snapshot_path, permissions).map_err(S3Error::transport)?;

    Ok(PreparedBody {
        source: PreparedSource::FileSnapshot(snapshot_path),
        length,
        sha256: hasher.finalize().into(),
    })
}

pub(crate) struct PreparedBody {
    source: PreparedSource,
    length: u64,
    sha256: [u8; 32],
}

enum PreparedSource {
    Bytes(Bytes),
    FileSnapshot(tempfile::TempPath),
    OneShot(Mutex<Option<UploadItems>>),
}

impl PreparedBody {
    pub(crate) fn length(&self) -> u64 {
        self.length
    }

    pub(crate) fn sha256_hex(&self) -> String {
        encode_hex(&self.sha256)
    }

    pub(crate) fn sha256_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.sha256)
    }

    pub(crate) fn is_replayable(&self) -> bool {
        !matches!(self.source, PreparedSource::OneShot(_))
    }

    pub(crate) async fn request_body(&self) -> Result<TransportBody, S3Error> {
        match &self.source {
            PreparedSource::Bytes(bytes) => Ok(TransportBody::new(
                Box::pin(futures_util::stream::once(std::future::ready(Ok(
                    bytes.clone()
                )))),
                self.length,
                self.sha256,
            )),
            PreparedSource::FileSnapshot(path) => {
                let file = File::open(path).await.map_err(S3Error::transport)?;
                let stream = ReaderStream::with_capacity(file.take(self.length), 64 * 1024);
                Ok(TransportBody::new(
                    Box::pin(stream),
                    self.length,
                    self.sha256,
                ))
            }
            PreparedSource::OneShot(stream) => {
                let stream = stream.lock().await.take().ok_or_else(|| {
                    S3Error::unsupported("a non-replayable upload body cannot be sent twice")
                })?;
                Ok(TransportBody::new(stream, self.length, self.sha256))
            }
        }
    }
}

/// The crate-private HTTP body used by the transport.
pub(crate) struct TransportBody {
    stream: UploadItems,
    remaining: u64,
    expected_sha256: [u8; 32],
    hasher: Sha256,
    finished: bool,
}

impl TransportBody {
    fn new(stream: UploadItems, length: u64, expected_sha256: [u8; 32]) -> Self {
        Self {
            stream,
            remaining: length,
            expected_sha256,
            hasher: Sha256::new(),
            finished: false,
        }
    }

    pub(crate) fn empty() -> Self {
        let digest = Sha256::digest([]).into();
        Self::new(Box::pin(futures_util::stream::empty()), 0, digest)
    }
}

impl Body for TransportBody {
    type Data = Bytes;
    type Error = S3Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.finished {
            return Poll::Ready(None);
        }

        loop {
            match self.stream.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    let Ok(chunk_length) = u64::try_from(chunk.len()) else {
                        self.finished = true;
                        return Poll::Ready(Some(Err(S3Error::integrity(
                            "upload chunk length does not fit in u64",
                        ))));
                    };
                    if chunk_length > self.remaining {
                        self.finished = true;
                        return Poll::Ready(Some(Err(S3Error::integrity(
                            "upload exceeded its declared content length",
                        ))));
                    }
                    self.remaining -= chunk_length;
                    self.hasher.update(&chunk);
                    if self.remaining == 0 {
                        let digest: [u8; 32] = self.hasher.clone().finalize().into();
                        if digest != self.expected_sha256 {
                            self.finished = true;
                            return Poll::Ready(Some(Err(S3Error::integrity(
                                "upload bytes did not match the signed payload digest",
                            ))));
                        }
                        self.finished = true;
                    }
                    return Poll::Ready(Some(Ok(Frame::data(chunk))));
                }
                Poll::Ready(Some(Err(error))) => {
                    self.finished = true;
                    return Poll::Ready(Some(Err(S3Error::transport(error))));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    if self.remaining != 0 {
                        return Poll::Ready(Some(Err(S3Error::integrity(
                            "upload ended before its declared content length",
                        ))));
                    }
                    let digest: [u8; 32] = self.hasher.clone().finalize().into();
                    return if digest == self.expected_sha256 {
                        Poll::Ready(None)
                    } else {
                        Poll::Ready(Some(Err(S3Error::integrity(
                            "upload bytes did not match the signed payload digest",
                        ))))
                    };
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.finished
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

/// A download stream that enforces idle timeout and content-length integrity.
pub struct ResponseStream {
    inner: DownloadItems,
    expected_length: Option<u64>,
    received: u64,
    idle_timeout: Duration,
    idle: Pin<Box<Sleep>>,
    deadline: Option<Pin<Box<Sleep>>>,
    finished: bool,
}

impl ResponseStream {
    pub(crate) fn new<S>(stream: S, expected_length: Option<u64>, idle_timeout: Duration) -> Self
    where
        S: Stream<Item = Result<Bytes, S3Error>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
            expected_length,
            received: 0,
            idle_timeout,
            idle: Box::pin(tokio::time::sleep(idle_timeout)),
            deadline: None,
            finished: false,
        }
    }

    pub(crate) fn with_deadline<S>(
        stream: S,
        expected_length: Option<u64>,
        idle_timeout: Duration,
        deadline: Instant,
    ) -> Self
    where
        S: Stream<Item = Result<Bytes, S3Error>> + Send + 'static,
    {
        let mut response = Self::new(stream, expected_length, idle_timeout);
        response.deadline = Some(Box::pin(tokio::time::sleep_until(deadline)));
        response
    }

    /// Streams the remaining body into `writer` with backpressure.
    ///
    /// # Errors
    ///
    /// Returns an error for transport, timeout, output, length, or integrity
    /// failures encountered while streaming the response.
    pub async fn write_to<W>(mut self, writer: &mut W) -> Result<u64, S3Error>
    where
        W: AsyncWrite + Unpin,
    {
        let mut written = 0_u64;
        while let Some(chunk) = self.next().await {
            let chunk = chunk?;
            writer.write_all(&chunk).await.map_err(S3Error::transport)?;
            written =
                written
                    .checked_add(u64::try_from(chunk.len()).map_err(|_| {
                        S3Error::integrity("download chunk length does not fit in u64")
                    })?)
                    .ok_or_else(|| S3Error::integrity("download length overflow"))?;
        }
        writer.flush().await.map_err(S3Error::transport)?;
        Ok(written)
    }

    /// Returns the number of bytes yielded so far.
    pub fn bytes_received(&self) -> u64 {
        self.received
    }
}

impl fmt::Debug for ResponseStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponseStream")
            .field("expected_length", &self.expected_length)
            .field("received", &self.received)
            .field("idle_timeout", &self.idle_timeout)
            .field("has_deadline", &self.deadline.is_some())
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl Stream for ResponseStream {
    type Item = Result<Bytes, S3Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }

        if self
            .deadline
            .as_mut()
            .is_some_and(|deadline| deadline.as_mut().poll(context).is_ready())
        {
            self.finished = true;
            return Poll::Ready(Some(Err(S3Error::timeout(
                crate::error::TimeoutPhase::Operation,
                "operation deadline expired while streaming the response body",
            ))));
        }

        match self.inner.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(chunk))) => {
                let idle_timeout = self.idle_timeout;
                self.idle.as_mut().reset(Instant::now() + idle_timeout);
                let Ok(chunk_length) = u64::try_from(chunk.len()) else {
                    self.finished = true;
                    return Poll::Ready(Some(Err(S3Error::integrity(
                        "download chunk length does not fit in u64",
                    ))));
                };
                let Some(received) = self.received.checked_add(chunk_length) else {
                    self.finished = true;
                    return Poll::Ready(Some(Err(S3Error::integrity("download length overflow"))));
                };
                self.received = received;
                if self
                    .expected_length
                    .is_some_and(|expected| self.received > expected)
                {
                    self.finished = true;
                    return Poll::Ready(Some(Err(S3Error::integrity(
                        "download exceeded the declared content length",
                    ))));
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.finished = true;
                if self
                    .expected_length
                    .is_some_and(|expected| self.received != expected)
                {
                    Poll::Ready(Some(Err(S3Error::integrity(
                        "download ended before the declared content length",
                    ))))
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => match self.idle.as_mut().poll(context) {
                Poll::Ready(()) => {
                    self.finished = true;
                    Poll::Ready(Some(Err(S3Error::timeout(
                        crate::error::TimeoutPhase::ResponseBody,
                        "download body was idle past its configured timeout",
                    ))))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use bytes::Bytes;
    use futures_util::{StreamExt, stream};
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    use super::{ByteStream, ResponseStream};

    #[tokio::test]
    async fn bytes_are_replayable_and_hashed() {
        let prepared = ByteStream::from_bytes(Bytes::from_static(b"hello"))
            .prepare()
            .await
            .expect("body prepares");
        assert!(prepared.is_replayable());
        assert_eq!(prepared.length(), 5);
        assert_eq!(
            prepared.sha256_hex(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let first = prepared
            .request_body()
            .await
            .expect("first body")
            .collect()
            .await
            .expect("first body is valid")
            .to_bytes();
        let second = prepared
            .request_body()
            .await
            .expect("second body")
            .collect()
            .await
            .expect("second body is valid")
            .to_bytes();
        assert_eq!(first, Bytes::from_static(b"hello"));
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn one_shot_stream_rejects_second_attempt() {
        let sha256 = Sha256::digest(b"x").into();
        let source = stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"x"))]);
        let prepared = ByteStream::from_stream(source, 1, sha256)
            .prepare()
            .await
            .expect("body prepares");
        assert!(!prepared.is_replayable());
        prepared
            .request_body()
            .await
            .expect("first body")
            .collect()
            .await
            .expect("first body is valid");
        assert!(prepared.request_body().await.is_err());
    }

    #[tokio::test]
    async fn file_retries_use_the_immutable_hashed_snapshot() {
        let mut original = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut original, b"signed bytes").unwrap();
        std::io::Write::flush(&mut original).unwrap();
        let prepared = ByteStream::from_path(original.path())
            .prepare()
            .await
            .expect("file body prepares");

        let mut changed = tokio::fs::File::create(original.path()).await.unwrap();
        changed.write_all(b"different bytes").await.unwrap();
        changed.flush().await.unwrap();

        for _ in 0..2 {
            let body = prepared
                .request_body()
                .await
                .expect("snapshot opens")
                .collect()
                .await
                .expect("snapshot body is valid")
                .to_bytes();
            assert_eq!(body, Bytes::from_static(b"signed bytes"));
        }
    }

    #[tokio::test]
    async fn one_shot_stream_enforces_declared_length_and_digest() {
        let too_long = stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"ab"))]);
        let prepared = ByteStream::from_stream(too_long, 1, Sha256::digest(b"a").into())
            .prepare()
            .await
            .unwrap();
        assert!(
            prepared
                .request_body()
                .await
                .unwrap()
                .collect()
                .await
                .is_err()
        );

        let wrong_digest = stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"x"))]);
        let prepared = ByteStream::from_stream(wrong_digest, 1, [0_u8; 32])
            .prepare()
            .await
            .unwrap();
        assert!(
            prepared
                .request_body()
                .await
                .unwrap()
                .collect()
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn response_detects_truncation() {
        let source = stream::iter([Ok(Bytes::from_static(b"abc"))]);
        let mut body = ResponseStream::new(source, Some(4), std::time::Duration::from_secs(1));
        assert!(body.next().await.expect("chunk").is_ok());
        assert!(body.next().await.expect("integrity result").is_err());
        assert!(body.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn response_enforces_operation_deadline() {
        let source = stream::pending::<Result<Bytes, crate::error::S3Error>>();
        let mut body = ResponseStream::with_deadline(
            source,
            None,
            std::time::Duration::from_secs(60),
            tokio::time::Instant::now() + std::time::Duration::from_secs(2),
        );
        let next = tokio::spawn(async move { body.next().await });
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        let error = next.await.unwrap().unwrap().unwrap_err();
        assert_eq!(
            error.timeout_phase(),
            Some(crate::error::TimeoutPhase::Operation)
        );
    }
}
