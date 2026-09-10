use std::cmp::min;
use std::time::Duration;

use http::{HeaderMap, Method, Response};
use hyper::body::Incoming;
use tokio::time::Instant;

use super::super::S3Client;
use super::response::{operation_timeout, protocol_error, service_error};
use super::retry::{controlled_region_redirect, parse_retry_after};
use super::signing::SignedRequestInput;
use crate::endpoint::EndpointUrl;
use crate::error::{RetryStopReason, S3Error, TimeoutPhase};
use crate::observer::{RequestEvent, RequestEventKind};
use crate::protocol::{ParsedS3Error, ProtocolError};
use crate::signing::{QueryParam, canonical_query, payload_sha256_hex};
use crate::stream::{ByteStream, PreparedBody};

#[derive(Clone, Copy)]
pub(in crate::client) struct OperationDeadline {
    at: Instant,
}

pub(in crate::client) struct CollectedSignedResponse {
    pub(in crate::client) headers: HeaderMap,
    pub(in crate::client) body: Vec<u8>,
}

type EmbeddedErrorParser = fn(&[u8], usize) -> Result<Option<ParsedS3Error>, ProtocolError>;

#[derive(Clone, Copy)]
enum SuccessMode {
    Streaming,
    CollectedXml {
        maximum: usize,
        embedded_error: EmbeddedErrorParser,
    },
}

enum SuccessfulResponse {
    Streaming(Response<Incoming>),
    Collected(CollectedSignedResponse),
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
        match self
            .send_signed_inner(
                method,
                target,
                query,
                headers,
                body,
                deadline,
                SuccessMode::Streaming,
            )
            .await?
        {
            SuccessfulResponse::Streaming(response) => Ok(response),
            SuccessfulResponse::Collected(_) => {
                unreachable!("streaming success mode returned a collected response")
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::client) async fn send_signed_collected_xml(
        &self,
        method: Method,
        target: EndpointUrl,
        query: &[(String, String)],
        headers: HeaderMap,
        body: Option<&PreparedBody>,
        deadline: &OperationDeadline,
        maximum: usize,
        embedded_error: EmbeddedErrorParser,
    ) -> Result<CollectedSignedResponse, S3Error> {
        match self
            .send_signed_inner(
                method,
                target,
                query,
                headers,
                body,
                deadline,
                SuccessMode::CollectedXml {
                    maximum,
                    embedded_error,
                },
            )
            .await?
        {
            SuccessfulResponse::Collected(response) => Ok(response),
            SuccessfulResponse::Streaming(_) => {
                unreachable!("collected success mode returned a streaming response")
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_signed_inner(
        &self,
        method: Method,
        target: EndpointUrl,
        query: &[(String, String)],
        headers: HeaderMap,
        body: Option<&PreparedBody>,
        deadline: &OperationDeadline,
        success_mode: SuccessMode,
    ) -> Result<SuccessfulResponse, S3Error> {
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
        let mut last_retry_cost = None;
        let mut pending_retry_permit: Option<super::super::RetryPermit> = None;

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
            if let Some(permit) = pending_retry_permit.take() {
                last_retry_cost = Some(permit.commit());
            }
            self.observe(RequestEvent {
                kind: RequestEventKind::AttemptStarted,
                method: method.as_str(),
                attempt: attempts_completed.saturating_add(1),
                error_category: None,
                retry_classification: None,
                retry_delay: None,
            });
            attempts_completed = attempts_completed.saturating_add(1);
            let attempt_timeout = min(self.inner.config.attempt_timeout(), remaining);
            let result = self.inner.transport.send(request, attempt_timeout).await;

            let (error, retry_after) = match result {
                Ok(response) if response.status().is_success() => match success_mode {
                    SuccessMode::Streaming => {
                        self.inner
                            .retry_quota
                            .replenish_after_success(last_retry_cost);
                        self.observe(RequestEvent {
                            kind: RequestEventKind::AttemptSucceeded,
                            method: method.as_str(),
                            attempt: attempts_completed,
                            error_category: None,
                            retry_classification: None,
                            retry_delay: None,
                        });
                        return Ok(SuccessfulResponse::Streaming(response));
                    }
                    SuccessMode::CollectedXml {
                        maximum,
                        embedded_error,
                    } => {
                        let response_headers = response.headers().clone();
                        let retry_after = parse_retry_after(&response_headers);
                        match self.collect_response(response, maximum, deadline).await {
                            Err(error) => (error, retry_after),
                            Ok(response_body) => match embedded_error(&response_body, maximum) {
                                Err(error) => (protocol_error(error), retry_after),
                                Ok(Some(parsed)) => (
                                    service_error(
                                        http::StatusCode::OK,
                                        &response_headers,
                                        Some(parsed),
                                    ),
                                    retry_after,
                                ),
                                Ok(None) => {
                                    self.inner
                                        .retry_quota
                                        .replenish_after_success(last_retry_cost);
                                    self.observe(RequestEvent {
                                        kind: RequestEventKind::AttemptSucceeded,
                                        method: method.as_str(),
                                        attempt: attempts_completed,
                                        error_category: None,
                                        retry_classification: None,
                                        retry_delay: None,
                                    });
                                    return Ok(SuccessfulResponse::Collected(
                                        CollectedSignedResponse {
                                            headers: response_headers,
                                            body: response_body,
                                        },
                                    ));
                                }
                            },
                        }
                    }
                },
                Ok(response)
                    if !region_redirected
                        && controlled_region_redirect(response.status(), response.headers())
                            .is_some() =>
                {
                    let region = controlled_region_redirect(response.status(), response.headers())
                        .expect("guard established a region redirect");
                    let policy = self.inner.config.retry_policy();
                    let service_authority = self.redirected_aws_authority(region);
                    match service_authority {
                        Some(service_authority)
                            if replayable
                                && attempts_completed < policy.max_attempts()
                                && started.elapsed() < policy.max_elapsed() =>
                        {
                            let authority = match self.inner.config.addressing_style() {
                                crate::config::AddressingStyle::Path => service_authority,
                                crate::config::AddressingStyle::VirtualHosted => {
                                    format!("{}.{service_authority}", self.inner.config.bucket())
                                }
                            };
                            target = target.with_authority(&authority)?;
                            signing_region = region.to_owned();
                            region_redirected = true;
                            self.observe(RequestEvent {
                                kind: RequestEventKind::RegionRedirected,
                                method: method.as_str(),
                                attempt: attempts_completed,
                                error_category: None,
                                retry_classification: Some(crate::RetryClassification::Retryable),
                                retry_delay: Some(Duration::ZERO),
                            });
                            continue;
                        }
                        _ => {
                            let retry_after = parse_retry_after(response.headers());
                            let error = self.response_error(response, deadline).await;
                            (error, retry_after)
                        }
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
            let classification = error.retry_classification();
            let stop_reason = if !replayable {
                Some(RetryStopReason::NonReplayable)
            } else if classification == crate::RetryClassification::Never {
                Some(RetryStopReason::NonRetryable)
            } else if attempts_completed >= policy.max_attempts() {
                Some(RetryStopReason::AttemptsExhausted)
            } else if elapsed >= policy.max_elapsed() {
                Some(RetryStopReason::ElapsedLimit)
            } else {
                None
            };
            if let Some(reason) = stop_reason {
                self.observe(RequestEvent {
                    kind: RequestEventKind::AttemptFailed,
                    method: method.as_str(),
                    attempt: attempts_completed,
                    error_category: Some(error.category()),
                    retry_classification: Some(classification),
                    retry_delay: None,
                });
                return Err(error.with_retry_context(attempts_completed, reason));
            }
            let Some(delay) =
                policy.delay_for(classification, attempts_completed, elapsed, retry_after)
            else {
                self.observe(RequestEvent {
                    kind: RequestEventKind::AttemptFailed,
                    method: method.as_str(),
                    attempt: attempts_completed,
                    error_category: Some(error.category()),
                    retry_classification: Some(classification),
                    retry_delay: None,
                });
                return Err(
                    error.with_retry_context(attempts_completed, RetryStopReason::ElapsedLimit)
                );
            };
            let remaining = match deadline.remaining() {
                Ok(remaining) => remaining,
                Err(_) => {
                    self.observe(RequestEvent {
                        kind: RequestEventKind::AttemptFailed,
                        method: method.as_str(),
                        attempt: attempts_completed,
                        error_category: Some(error.category()),
                        retry_classification: Some(classification),
                        retry_delay: None,
                    });
                    return Err(
                        error.with_retry_context(attempts_completed, RetryStopReason::Deadline)
                    );
                }
            };
            if delay >= remaining {
                self.observe(RequestEvent {
                    kind: RequestEventKind::AttemptFailed,
                    method: method.as_str(),
                    attempt: attempts_completed,
                    error_category: Some(error.category()),
                    retry_classification: Some(classification),
                    retry_delay: None,
                });
                return Err(error.with_retry_context(attempts_completed, RetryStopReason::Deadline));
            }
            let Some(retry_permit) = self.inner.retry_quota.acquire(classification) else {
                self.observe(RequestEvent {
                    kind: RequestEventKind::AttemptFailed,
                    method: method.as_str(),
                    attempt: attempts_completed,
                    error_category: Some(error.category()),
                    retry_classification: Some(classification),
                    retry_delay: None,
                });
                return Err(
                    error.with_retry_context(attempts_completed, RetryStopReason::RetryQuota)
                );
            };
            pending_retry_permit = Some(retry_permit);
            self.observe(RequestEvent {
                kind: RequestEventKind::RetryScheduled,
                method: method.as_str(),
                attempt: attempts_completed,
                error_category: Some(error.category()),
                retry_classification: Some(classification),
                retry_delay: Some(delay),
            });
            tokio::time::sleep(delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future as _, poll_fn};
    use std::sync::Arc;
    use std::task::Poll;
    use std::time::Duration;

    use http::{HeaderMap, Method};

    use super::super::signing::SignedRequestInput;
    use super::OperationDeadline;
    use crate::client::S3Client;
    use crate::config::S3Config;
    use crate::credentials::{Credentials, StaticCredentialsProvider};
    use crate::endpoint::Endpoint;
    use crate::error::{ErrorCategory, TimeoutPhase};
    use crate::signing::payload_sha256_hex;
    use crate::stream::ByteStream;

    #[tokio::test]
    async fn aws_region_correction_resigns_the_same_custom_headers_for_the_new_authority() {
        let credentials = Credentials::new("access", "secret", None).unwrap();
        let config = S3Config::builder()
            .endpoint(Endpoint::for_aws_region("us-east-1").unwrap())
            .region("us-east-1")
            .bucket("bucket")
            .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
            .build()
            .unwrap();
        let client = S3Client::new(config).unwrap();
        let initial_target = client.operation_target(Some("key")).unwrap();
        let corrected_authority = client.redirected_aws_authority("us-west-2").unwrap();
        let corrected_target = initial_target.with_authority(&corrected_authority).unwrap();
        let mut caller_headers = HeaderMap::new();
        caller_headers.append("x-example", "one".parse().unwrap());
        caller_headers.append("x-example", "two".parse().unwrap());
        let payload_hash = payload_sha256_hex(&[]);

        let initial = client
            .signed_request(SignedRequestInput {
                method: Method::GET,
                target: &initial_target,
                query: &[],
                headers: caller_headers.clone(),
                body: None,
                payload_hash: &payload_hash,
                signing_region: "us-east-1",
            })
            .await
            .unwrap();
        let corrected = client
            .signed_request(SignedRequestInput {
                method: Method::GET,
                target: &corrected_target,
                query: &[],
                headers: caller_headers.clone(),
                body: None,
                payload_hash: &payload_hash,
                signing_region: "us-west-2",
            })
            .await
            .unwrap();

        for (request, region, authority) in [
            (&initial, "us-east-1", "s3.us-east-1.amazonaws.com"),
            (&corrected, "us-west-2", "s3.us-west-2.amazonaws.com"),
        ] {
            assert_eq!(
                request
                    .headers()
                    .get_all("x-example")
                    .iter()
                    .map(|value| value.to_str().unwrap())
                    .collect::<Vec<_>>(),
                ["one", "two"]
            );
            assert_eq!(request.uri().authority().unwrap().as_str(), authority);
            let authorization = request.headers()[http::header::AUTHORIZATION]
                .to_str()
                .unwrap();
            assert!(authorization.contains("Credential=access/20"));
            assert!(authorization.contains(&format!("/{region}/s3/aws4_request")));
            let signed_headers = authorization
                .split("SignedHeaders=")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap();
            assert!(signed_headers.split(';').any(|name| name == "x-example"));
        }
        assert_eq!(caller_headers.get_all("x-example").iter().count(), 2);
    }

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
