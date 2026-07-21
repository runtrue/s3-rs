use std::cmp::min;
use std::time::{Duration, SystemTime};

use http::header::{AUTHORIZATION, CONTENT_LENGTH, DATE, HOST, RETRY_AFTER};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode};
use http_body_util::BodyExt as _;
use hyper::body::{Body, Incoming};
use secrecy::ExposeSecret;
use time::OffsetDateTime;
use tokio::time::Instant;

use super::S3Client;
use crate::endpoint::EndpointUrl;
use crate::error::{ErrorCategory, RetryClassification, S3Error, TimeoutPhase};
use crate::protocol::{ParsedS3Error, ProtocolError, parse_s3_error};
use crate::signing::{
    Header, HeaderSigningRequest, QueryParam, SigningCredentials, SigningPath, SigningScope,
    canonical_query, payload_sha256_hex, sign_headers,
};
use crate::stream::{PreparedBody, TransportBody};

pub(super) struct OperationDeadline {
    at: Instant,
}

struct SignedRequestInput<'a> {
    method: Method,
    target: &'a EndpointUrl,
    query: &'a [QueryParam<'a>],
    headers: HeaderMap,
    body: Option<&'a PreparedBody>,
    payload_hash: &'a str,
    signing_region: &'a str,
}

impl OperationDeadline {
    pub(super) fn new(timeout: Duration) -> Self {
        Self {
            at: Instant::now() + timeout,
        }
    }

    pub(super) fn instant(&self) -> Instant {
        self.at
    }

    fn remaining(&self) -> Result<Duration, S3Error> {
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
}

impl S3Client {
    pub(super) fn deadline(&self) -> OperationDeadline {
        OperationDeadline::new(self.inner.config.operation_timeout())
    }

    pub(super) async fn send_signed(
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
            let remaining = deadline.remaining()?;
            let request = self
                .signed_request(SignedRequestInput {
                    method: method.clone(),
                    target: &target,
                    query: &query_refs,
                    headers: headers.clone(),
                    body,
                    payload_hash: &payload_hash,
                    signing_region: &signing_region,
                })
                .await?;
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
                    if !self.uses_standard_aws_endpoint() {
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

    async fn signed_request(
        &self,
        input: SignedRequestInput<'_>,
    ) -> Result<http::Request<TransportBody>, S3Error> {
        let SignedRequestInput {
            method,
            target,
            query,
            mut headers,
            body,
            payload_hash,
            signing_region,
        } = input;
        headers.insert(
            HOST,
            HeaderValue::from_str(target.authority())
                .map_err(|_| S3Error::configuration("endpoint authority is not a valid header"))?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-content-sha256"),
            HeaderValue::from_str(payload_hash)
                .map_err(|_| S3Error::configuration("payload digest is not a valid header"))?,
        );
        if let Some(body) = body {
            headers.insert(CONTENT_LENGTH, HeaderValue::from(body.length()));
        }

        let credentials = self
            .inner
            .config
            .credentials_provider()
            .provide_credentials()
            .await?;
        if credentials.expires_by(OffsetDateTime::now_utc()) {
            return Err(S3Error::new(
                ErrorCategory::Authentication,
                "credential provider returned expired credentials",
                RetryClassification::Never,
            ));
        }

        let owned_headers = headers
            .iter()
            .map(|(name, value)| {
                value
                    .to_str()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
                    .map_err(|_| S3Error::configuration("a signed header contains non-ASCII bytes"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signing_headers = owned_headers
            .iter()
            .map(|(name, value)| Header::new(name, value))
            .collect::<Vec<_>>();
        let secret = credentials.secret_access_key().expose_secret();
        let session_token = credentials.session_token().map(ExposeSecret::expose_secret);
        let signing_credentials = SigningCredentials::new(
            credentials.access_key_id(),
            secret.as_bytes(),
            session_token,
        );
        let path = target
            .path_and_query()
            .split_once('?')
            .map_or(target.path_and_query(), |(path, _)| path);
        let signing_request = HeaderSigningRequest {
            method: method.as_str(),
            uri_path: SigningPath::encoded(path),
            query,
            headers: &signing_headers,
            payload_hash,
        };
        let signed: crate::signing::HeaderSigningOutput = sign_headers(
            &signing_credentials,
            SigningScope::new(signing_region, "s3"),
            &signing_request,
            OffsetDateTime::now_utc(),
        )
        .map_err(|error| {
            S3Error::new(
                ErrorCategory::Authentication,
                "request signing failed",
                RetryClassification::Never,
            )
            .with_source(error)
        })?;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(signed.authorization())
                .map_err(|_| S3Error::configuration("generated authorization header is invalid"))?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-date"),
            HeaderValue::from_str(signed.amz_date())
                .map_err(|_| S3Error::configuration("generated signing date is invalid"))?,
        );
        if let Some(token) = signed.security_token() {
            headers.insert(
                HeaderName::from_static("x-amz-security-token"),
                HeaderValue::from_str(token).map_err(|_| {
                    S3Error::configuration("generated session token header is invalid")
                })?,
            );
        }

        let request_body = match body {
            Some(body) => body.request_body().await?,
            None => TransportBody::empty(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(target.request_uri())
            .body(request_body)
            .map_err(|error| {
                S3Error::configuration("signed request could not be constructed").with_source(error)
            })?;
        *request.headers_mut() = headers;
        Ok(request)
    }

    fn uses_standard_aws_endpoint(&self) -> bool {
        let endpoint = self.inner.config.endpoint().url();
        if endpoint.scheme() != "https" {
            return false;
        }
        let authority = endpoint.authority();
        let host = authority
            .split_once(':')
            .map_or(authority, |(host, _)| host);
        host == "s3.amazonaws.com" || (host.starts_with("s3.") && host.ends_with(".amazonaws.com"))
    }

    async fn response_error(
        &self,
        response: Response<Incoming>,
        deadline: &OperationDeadline,
    ) -> S3Error {
        let status = response.status();
        let headers = response.headers().clone();
        let body = match self
            .collect_response(
                response,
                self.inner.config.max_error_response_size(),
                deadline,
            )
            .await
        {
            Ok(body) => body,
            Err(error) => return error,
        };
        let parsed = parse_s3_error(&body, self.inner.config.max_error_response_size()).ok();
        service_error(status, &headers, parsed)
    }

    pub(super) async fn collect_response(
        &self,
        response: Response<Incoming>,
        maximum: usize,
        deadline: &OperationDeadline,
    ) -> Result<Vec<u8>, S3Error> {
        if response
            .body()
            .size_hint()
            .upper()
            .is_some_and(|length| length > u64::try_from(maximum).unwrap_or(u64::MAX))
        {
            return Err(oversized_response(maximum));
        }
        collect_incoming(
            response.into_body(),
            maximum,
            self.inner.config.idle_body_timeout(),
            deadline,
        )
        .await
    }
}

async fn collect_incoming(
    mut body: Incoming,
    maximum: usize,
    idle_timeout: Duration,
    deadline: &OperationDeadline,
) -> Result<Vec<u8>, S3Error> {
    let mut output = Vec::with_capacity(maximum.min(8 * 1024));
    loop {
        let wait = min(idle_timeout, deadline.remaining()?);
        let frame = tokio::time::timeout(wait, body.frame())
            .await
            .map_err(|_| {
                S3Error::timeout(
                    TimeoutPhase::ResponseBody,
                    "response body was idle past its configured timeout",
                )
            })?;
        let Some(frame) = frame else {
            return Ok(output);
        };
        let frame = frame.map_err(S3Error::transport)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        let new_length = output
            .len()
            .checked_add(data.len())
            .ok_or_else(|| oversized_response(maximum))?;
        if new_length > maximum {
            return Err(oversized_response(maximum));
        }
        output.extend_from_slice(&data);
    }
}

fn oversized_response(maximum: usize) -> S3Error {
    S3Error::new(
        ErrorCategory::OversizedResponse,
        format!("response exceeded the configured {maximum}-byte limit"),
        RetryClassification::Never,
    )
}

pub(super) fn service_error(
    status: StatusCode,
    headers: &HeaderMap,
    parsed: Option<ParsedS3Error>,
) -> S3Error {
    let code = parsed.as_ref().and_then(|error| error.code.clone());
    let category = classify_service_error(status, code.as_deref());
    let retry = match category {
        ErrorCategory::Throttling => RetryClassification::Throttled,
        ErrorCategory::Server => RetryClassification::Retryable,
        _ => RetryClassification::Never,
    };
    let message = parsed
        .as_ref()
        .and_then(|error| error.message.as_deref())
        .and_then(sanitize_service_message)
        .unwrap_or_else(|| format!("S3 request failed with HTTP status {status}"));
    let request_id = header_text(headers, "x-amz-request-id")
        .or_else(|| parsed.as_ref().and_then(|error| error.request_id.clone()));
    let host_id = header_text(headers, "x-amz-id-2")
        .or_else(|| parsed.as_ref().and_then(|error| error.host_id.clone()));
    let mut error = S3Error::new(category, message, retry).with_service_details(
        code.clone(),
        status,
        request_id,
        host_id,
    );
    if matches!(
        code.as_deref(),
        Some("RequestTimeTooSkewed" | "RequestExpired")
    ) && let Some(server_time) = headers
        .get(DATE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| httpdate::parse_http_date(value).ok())
        .map(OffsetDateTime::from)
    {
        error = error.with_clock_skew(server_time, OffsetDateTime::now_utc());
    }
    error
}

fn classify_service_error(status: StatusCode, code: Option<&str>) -> ErrorCategory {
    match code {
        Some("InvalidAccessKeyId" | "SignatureDoesNotMatch" | "ExpiredToken" | "InvalidToken") => {
            ErrorCategory::Authentication
        }
        Some("AccessDenied" | "AllAccessDisabled") => ErrorCategory::Authorization,
        Some("NoSuchKey" | "NoSuchBucket" | "NoSuchUpload" | "NotFound") => ErrorCategory::NotFound,
        Some("PreconditionFailed" | "ConditionalRequestConflict") => ErrorCategory::Precondition,
        Some("SlowDown" | "Throttling" | "ThrottlingException") => ErrorCategory::Throttling,
        _ if status == StatusCode::UNAUTHORIZED => ErrorCategory::Authentication,
        _ if status == StatusCode::FORBIDDEN => ErrorCategory::Authorization,
        _ if status == StatusCode::NOT_FOUND => ErrorCategory::NotFound,
        _ if status == StatusCode::CONFLICT => ErrorCategory::Conflict,
        _ if status == StatusCode::PRECONDITION_FAILED => ErrorCategory::Precondition,
        _ if status == StatusCode::TOO_MANY_REQUESTS => ErrorCategory::Throttling,
        _ if status.is_server_error() => ErrorCategory::Server,
        _ => ErrorCategory::InvalidResponse,
    }
}

fn sanitize_service_message(message: &str) -> Option<String> {
    if message.is_empty()
        || message.len() > 512
        || message.contains("X-Amz-")
        || message.to_ascii_lowercase().contains("authorization")
        || message.to_ascii_lowercase().contains("token")
        || !message.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character.is_ascii_whitespace()
                || matches!(
                    character,
                    '.' | ',' | ':' | ';' | '_' | '-' | '\'' | '(' | ')'
                )
        })
    {
        return None;
    }
    Some(
        message
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn header_text(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 1024
                && value.bytes().all(|byte| !byte.is_ascii_control())
        })
        .map(str::to_owned)
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    retry_at.duration_since(SystemTime::now()).ok()
}

fn controlled_region_redirect(status: StatusCode, headers: &HeaderMap) -> Option<&str> {
    if !matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
            | StatusCode::BAD_REQUEST
    ) {
        return None;
    }
    headers
        .get("x-amz-bucket-region")?
        .to_str()
        .ok()
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(super) fn protocol_error(error: ProtocolError) -> S3Error {
    match error {
        ProtocolError::Oversized { maximum, .. } => oversized_response(maximum),
        other => S3Error::invalid_response("S3 returned malformed XML").with_source(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_message_filter_rejects_signing_material() {
        assert!(sanitize_service_message("ordinary S3 error").is_some());
        assert!(sanitize_service_message("X-Amz-Signature=secret").is_none());
        assert!(sanitize_service_message("authorization leaked").is_none());
    }

    #[test]
    fn status_and_code_classification_is_structured() {
        assert_eq!(
            classify_service_error(StatusCode::BAD_REQUEST, Some("SlowDown")),
            ErrorCategory::Throttling
        );
        assert_eq!(
            classify_service_error(StatusCode::FORBIDDEN, None),
            ErrorCategory::Authorization
        );
    }
}
