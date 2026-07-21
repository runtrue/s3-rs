use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, Sleep};

use crate::error::S3Error;

type DownloadItems = Pin<Box<dyn Stream<Item = Result<Bytes, S3Error>> + Send + 'static>>;

/// A download stream that enforces idle timeout, content length, and any
/// supported full-object checksum returned by S3.
pub struct ResponseStream {
    inner: DownloadItems,
    expected_length: Option<u64>,
    expected_sha256: Option<[u8; 32]>,
    sha256: Option<Sha256>,
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
            expected_sha256: None,
            sha256: None,
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
        expected_sha256: Option<[u8; 32]>,
        idle_timeout: Duration,
        deadline: Instant,
    ) -> Self
    where
        S: Stream<Item = Result<Bytes, S3Error>> + Send + 'static,
    {
        let mut response = Self::new(stream, expected_length, idle_timeout);
        response.expected_sha256 = expected_sha256;
        response.sha256 = expected_sha256.map(|_| Sha256::new());
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
            .field("verifies_sha256", &self.expected_sha256.is_some())
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

        if self.idle.as_mut().poll(context).is_ready() {
            self.finished = true;
            return Poll::Ready(Some(Err(S3Error::timeout(
                crate::error::TimeoutPhase::ResponseBody,
                "download body was idle past its configured timeout",
            ))));
        }

        match self.inner.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(chunk))) => {
                if chunk.is_empty() {
                    context.waker().wake_by_ref();
                    return Poll::Pending;
                }
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
                if let Some(hasher) = &mut self.sha256 {
                    hasher.update(&chunk);
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
                } else if let Some(expected) = self.expected_sha256 {
                    let actual: [u8; 32] = self
                        .sha256
                        .take()
                        .expect("SHA-256 state accompanies an expected digest")
                        .finalize()
                        .into();
                    if actual == expected {
                        Poll::Ready(None)
                    } else {
                        Poll::Ready(Some(Err(S3Error::integrity(
                            "download bytes did not match the returned SHA-256 checksum",
                        ))))
                    }
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::{StreamExt, stream};
    use sha2::{Digest, Sha256};

    use super::ResponseStream;

    #[tokio::test]
    async fn response_detects_truncation() {
        let source = stream::iter([Ok(Bytes::from_static(b"abc"))]);
        let mut body = ResponseStream::new(source, Some(4), std::time::Duration::from_secs(1));
        assert!(body.next().await.expect("chunk").is_ok());
        assert!(body.next().await.expect("integrity result").is_err());
        assert!(body.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn empty_download_chunks_do_not_count_as_progress() {
        let source = stream::once(async { Ok(Bytes::new()) })
            .chain(stream::pending::<Result<Bytes, crate::error::S3Error>>());
        let mut body = ResponseStream::new(source, None, std::time::Duration::from_secs(2));
        let next = tokio::spawn(async move { body.next().await });

        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        let error = next.await.unwrap().unwrap().unwrap_err();
        assert_eq!(
            error.timeout_phase(),
            Some(crate::error::TimeoutPhase::ResponseBody)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn response_enforces_operation_deadline() {
        let source = stream::pending::<Result<Bytes, crate::error::S3Error>>();
        let mut body = ResponseStream::with_deadline(
            source,
            None,
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

    #[tokio::test]
    async fn response_verifies_expected_sha256_at_end_of_stream() {
        let source = stream::iter([Ok(Bytes::from_static(b"abc"))]);
        let mut body = ResponseStream::with_deadline(
            source,
            Some(3),
            Some(Sha256::digest(b"abc").into()),
            std::time::Duration::from_secs(1),
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert_eq!(
            body.next().await.unwrap().unwrap(),
            Bytes::from_static(b"abc")
        );
        assert!(body.next().await.is_none());

        let source = stream::iter([Ok(Bytes::from_static(b"abc"))]);
        let mut body = ResponseStream::with_deadline(
            source,
            Some(3),
            Some(Sha256::digest(b"different").into()),
            std::time::Duration::from_secs(1),
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(body.next().await.unwrap().is_ok());
        let error = body.next().await.unwrap().unwrap_err();
        assert_eq!(error.category(), crate::error::ErrorCategory::Integrity);
        assert_eq!(
            error.retry_classification(),
            crate::error::RetryClassification::Never
        );
    }
}
