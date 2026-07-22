use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use base64::Engine as _;
use bytes::Bytes;
use futures_core::Stream;
use hyper::body::{Body, Frame, SizeHint};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio_util::io::ReaderStream;

use crate::error::S3Error;
use crate::stream::FileSnapshot;

type UploadItems = Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send + 'static>>;

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
                let sha256 = cooperative_sha256(&bytes).await;
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

async fn cooperative_sha256(bytes: &[u8]) -> [u8; 32] {
    const CHUNK_SIZE: usize = 1024 * 1024;

    let mut hasher = Sha256::new();
    for chunk in bytes.chunks(CHUNK_SIZE) {
        hasher.update(chunk);
        tokio::task::yield_now().await;
    }
    hasher.finalize().into()
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
    // Every attempt opens the same private snapshot, so later changes to the
    // caller's path cannot change already signed bytes.
    let snapshot = FileSnapshot::create(path, true).await?;
    let length = snapshot.length();
    let sha256 = snapshot
        .sha256()
        .ok_or_else(|| S3Error::integrity("upload snapshot digest was not calculated"))?;

    Ok(PreparedBody {
        source: PreparedSource::FileSnapshot(snapshot),
        length,
        sha256,
    })
}

pub(crate) struct PreparedBody {
    source: PreparedSource,
    length: u64,
    sha256: [u8; 32],
}

enum PreparedSource {
    Bytes(Bytes),
    FileSnapshot(FileSnapshot),
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
            PreparedSource::FileSnapshot(snapshot) => {
                let file = snapshot.open().await?;
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
    final_chunk: Option<Bytes>,
    finished: bool,
}

impl TransportBody {
    fn new(stream: UploadItems, length: u64, expected_sha256: [u8; 32]) -> Self {
        Self {
            stream,
            remaining: length,
            expected_sha256,
            hasher: Sha256::new(),
            final_chunk: None,
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
            if self.final_chunk.is_some() {
                match self.stream.as_mut().poll_next(context) {
                    Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() => continue,
                    Poll::Ready(Some(Ok(_))) => {
                        self.finished = true;
                        self.remaining = 0;
                        self.final_chunk = None;
                        return Poll::Ready(Some(Err(S3Error::integrity(
                            "upload exceeded its declared content length",
                        ))));
                    }
                    Poll::Ready(Some(Err(error))) => {
                        self.finished = true;
                        self.remaining = 0;
                        self.final_chunk = None;
                        return Poll::Ready(Some(Err(S3Error::transport(error))));
                    }
                    Poll::Ready(None) => {
                        let digest: [u8; 32] = self.hasher.clone().finalize().into();
                        if digest != self.expected_sha256 {
                            self.finished = true;
                            self.remaining = 0;
                            self.final_chunk = None;
                            return Poll::Ready(Some(Err(S3Error::integrity(
                                "upload bytes did not match the signed payload digest",
                            ))));
                        }
                        self.finished = true;
                        self.remaining = 0;
                        let chunk = self
                            .final_chunk
                            .take()
                            .expect("final upload chunk was established");
                        return Poll::Ready(Some(Ok(Frame::data(chunk))));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            match self.stream.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    let Ok(chunk_length) = u64::try_from(chunk.len()) else {
                        self.finished = true;
                        self.remaining = 0;
                        return Poll::Ready(Some(Err(S3Error::integrity(
                            "upload chunk length does not fit in u64",
                        ))));
                    };
                    if chunk_length > self.remaining {
                        self.finished = true;
                        self.remaining = 0;
                        return Poll::Ready(Some(Err(S3Error::integrity(
                            "upload exceeded its declared content length",
                        ))));
                    }
                    self.hasher.update(&chunk);
                    if chunk_length == self.remaining {
                        self.final_chunk = Some(chunk);
                        continue;
                    }
                    self.remaining -= chunk_length;
                    return Poll::Ready(Some(Ok(Frame::data(chunk))));
                }
                Poll::Ready(Some(Err(error))) => {
                    self.finished = true;
                    self.remaining = 0;
                    return Poll::Ready(Some(Err(S3Error::transport(error))));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    if self.remaining != 0 {
                        self.remaining = 0;
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

#[cfg(test)]
mod tests {
    use std::future::{Future as _, poll_fn};
    use std::io;
    use std::task::Poll;

    use bytes::Bytes;
    use futures_util::stream;
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    use super::ByteStream;

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
    async fn hashing_large_in_memory_bodies_is_cooperative() {
        let mut preparation =
            Box::pin(ByteStream::from_bytes(vec![0_u8; 2 * 1024 * 1024]).prepare());

        poll_fn(|context| match preparation.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("large body hashing completed in one cooperative poll"),
        })
        .await;
        preparation.await.expect("body finishes preparing");
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

        let trailing_chunk = stream::iter([
            Ok::<_, io::Error>(Bytes::from_static(b"a")),
            Ok::<_, io::Error>(Bytes::from_static(b"b")),
        ]);
        let prepared = ByteStream::from_stream(trailing_chunk, 1, Sha256::digest(b"a").into())
            .prepare()
            .await
            .unwrap();
        let error = prepared
            .request_body()
            .await
            .unwrap()
            .collect()
            .await
            .unwrap_err();
        assert_eq!(error.category(), crate::error::ErrorCategory::Integrity);
    }
}
