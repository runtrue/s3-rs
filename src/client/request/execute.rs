use std::cmp::min;
use std::time::Duration;

use http::{HeaderMap, Method, Response};
use hyper::body::Incoming;
use tokio::time::Instant;

use super::super::S3Client;
use super::response::operation_timeout;
use super::retry::{controlled_region_redirect, parse_retry_after};
use super::signing::SignedRequestInput;
use crate::endpoint::EndpointUrl;
use crate::error::{S3Error, TimeoutPhase};
use crate::signing::{QueryParam, canonical_query, payload_sha256_hex};
use crate::stream::{ByteStream, PreparedBody};

#[derive(Clone, Copy)]
pub(in crate::client) struct OperationDeadline {
    at: Instant,
}

impl OperationDeadline {
    pub(in crate::client) fn new(timeout: Duration) -> Self {
        Self {
            at: Instant::now() + timeout,
        }
    }

    pub(in crate::client) fn instant(&self) -> Instant {
        self.at
    }

    pub(super) fn remaining(&self) -> Result<Duration, S3Error> {
        self.at
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                S3Error::timeout(
                    TimeoutPhase::Operation,
                    "S3 operation exceeded its overall deadline",
                )
            })
    }

    pub(in crate::client) async fn prepare_body(
        &self,
        body: ByteStream,
    ) -> Result<PreparedBody, S3Error> {
        tokio::time::timeout_at(self.instant(), body.prepare())
            .await
            .map_err(|_| operation_timeout())?
    }
}

impl S3Client {
    pub(in crate::client) fn deadline(&self) -> OperationDeadline {
        OperationDeadline::new(self.inner.config.operation_timeout())
    }

    pub(in crate::client) async fn send_signed(
        &self,
        method: Method,
        target: EndpointUrl,
        query: &[(String, String)],
        headers: HeaderMap,
        body: Option<&PreparedBody>,
        deadline: &OperationDeadline,
    ) -> Result<Response<Incoming>, S3Error> {
        let query_refs = query
            .iter()
            .map(|(name, value)| QueryParam::new(name, value))
            .collect::<Vec<_>>();
        let encoded_query = canonical_query(&query_refs);
        let mut target = target.with_query(&encoded_query)?;
        let mut signing_region = self.inner.config.region().to_owned();
        let mut region_redirected = false;
        let replayable = body.is_none_or(PreparedBody::is_replayable);
        let payload_hash = body.map_or_else(|| payload_sha256_hex(&[]), PreparedBody::sha256_hex);
        let started = Instant::now();
        let mut attempts_completed = 0_u32;

        loop {
            deadline.remaining()?;
            let request = tokio::time::timeout_at(
                deadline.instant(),
                self.signed_request(SignedRequestInput {
                    method: method.clone(),
                    target: &target,
                    query: &query_refs,
                    headers: headers.clone(),
                    body,
                    payload_hash: &payload_hash,
                    signing_region: &signing_region,
                }),
            )
            .await
            .map_err(|_| operation_timeout())??;
            let remaining = deadline.remaining()?;
            attempts_completed = attempts_completed.saturating_add(1);
            let attempt_timeout = min(self.inner.config.attempt_timeout(), remaining);
            let result = self.inner.transport.send(request, attempt_timeout).await;

            let (error, retry_after) = match result {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response)
                    if !region_redirected
                        && controlled_region_redirect(response.status(), response.headers())
                            .is_some() =>
                {
                    let region = controlled_region_redirect(response.status(), response.headers())
                        .expect("guard established a region redirect");
                    let policy = self.inner.config.retry_policy();
                    if !self.uses_standard_aws_endpoint()
                        || !replayable
                        || attempts_completed >= policy.max_attempts()
                        || started.elapsed() >= policy.max_elapsed()
                    {
                        let retry_after = parse_retry_after(response.headers());
                        let error = self.response_error(response, deadline).await;
                        (error, retry_after)
                    } else {
                        let authority = match self.inner.config.addressing_style() {
                            crate::config::AddressingStyle::Path => {
                                format!("s3.{region}.amazonaws.com")
                            }
                            crate::config::AddressingStyle::VirtualHosted => {
                                format!("{}.s3.{region}.amazonaws.com", self.inner.config.bucket())
                            }
                        };
                        target = target.with_authority(&authority)?;
                        signing_region = region.to_owned();
                        region_redirected = true;
                        continue;
                    }
                }
                Ok(response) => {
                    let retry_after = parse_retry_after(response.headers());
                    let error = self.response_error(response, deadline).await;
                    (error, retry_after)
                }
                Err(error) => (error, None),
            };

            let elapsed = started.elapsed();
            let policy = self.inner.config.retry_policy();
            if !replayable
                || !policy.permits_retry(error.retry_classification(), attempts_completed, elapsed)
            {
                return Err(error);
            }
            let Some(delay) = policy.delay(attempts_completed, elapsed, retry_after) else {
                return Err(error);
            };
            if delay >= deadline.remaining()? {
                return Err(error);
            }
            tokio::time::sleep(delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future as _, poll_fn};
    use std::task::Poll;
    use std::time::Duration;

    use super::OperationDeadline;
    use crate::error::{ErrorCategory, TimeoutPhase};
    use crate::stream::ByteStream;

    #[tokio::test(start_paused = true)]
    async fn body_preparation_obeys_the_operation_deadline() {
        let deadline = OperationDeadline::new(Duration::from_secs(1));
        let mut preparation =
            Box::pin(deadline.prepare_body(ByteStream::from_bytes(vec![0_u8; 4 * 1024 * 1024])));
        poll_fn(|context| match preparation.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("large body preparation completed in one poll"),
        })
        .await;

        tokio::time::advance(Duration::from_secs(1)).await;
        let Err(error) = preparation.await else {
            panic!("body preparation exceeded its operation deadline");
        };
        assert_eq!(error.category(), ErrorCategory::Timeout);
        assert_eq!(error.timeout_phase(), Some(TimeoutPhase::Operation));
    }
}
