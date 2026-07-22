use std::cmp::min;
use std::time::Duration;

use http::header::DATE;
use http::{HeaderMap, Response, StatusCode};
use http_body_util::BodyExt as _;
use hyper::body::{Body, Incoming};
use time::OffsetDateTime;
use tokio::time::Instant;

use super::super::S3Client;
use super::execute::OperationDeadline;
use crate::error::{ErrorCategory, RetryClassification, S3Error, TimeoutPhase};
use crate::protocol::{ParsedS3Error, ProtocolError, parse_s3_error};

impl S3Client {
    pub(super) async fn response_error(
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

    pub(in crate::client) async fn collect_response(
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

    pub(in crate::client) async fn drain_success_response(
        &self,
        response: Response<Incoming>,
        deadline: &OperationDeadline,
    ) -> Result<(), S3Error> {
        const MAXIMUM: usize = 8 * 1024;
        self.collect_response(response, MAXIMUM, deadline)
            .await
            .map(|_| ())
    }
}

pub(super) fn operation_timeout() -> S3Error {
    S3Error::timeout(
        TimeoutPhase::Operation,
        "S3 operation exceeded its overall deadline",
    )
}

async fn collect_incoming(
    mut body: Incoming,
    maximum: usize,
    idle_timeout: Duration,
    deadline: &OperationDeadline,
) -> Result<Vec<u8>, S3Error> {
    let mut output = Vec::with_capacity(maximum.min(8 * 1024));
    let mut idle_deadline = Instant::now() + idle_timeout;
    loop {
        let operation_deadline = deadline.instant();
        if operation_deadline <= Instant::now() {
            return Err(operation_timeout());
        }
        if idle_deadline <= Instant::now() {
            return Err(S3Error::timeout(
                TimeoutPhase::ResponseBody,
                "response body was idle past its configured timeout",
            ));
        }
        let wait_until = min(operation_deadline, idle_deadline);
        let frame = tokio::time::timeout_at(wait_until, body.frame())
            .await
            .map_err(|_| {
                if operation_deadline <= idle_deadline {
                    S3Error::timeout(
                        TimeoutPhase::Operation,
                        "S3 operation exceeded its overall deadline",
                    )
                } else {
                    S3Error::timeout(
                        TimeoutPhase::ResponseBody,
                        "response body was idle past its configured timeout",
                    )
                }
            })?;
        let Some(frame) = frame else {
            return Ok(output);
        };
        let frame = frame.map_err(S3Error::transport)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if data.is_empty() {
            tokio::task::yield_now().await;
            continue;
        }
        let new_length = output
            .len()
            .checked_add(data.len())
            .ok_or_else(|| oversized_response(maximum))?;
        if new_length > maximum {
            return Err(oversized_response(maximum));
        }
        output.extend_from_slice(&data);
        idle_deadline = Instant::now() + idle_timeout;
    }
}

fn oversized_response(maximum: usize) -> S3Error {
    S3Error::new(
        ErrorCategory::OversizedResponse,
        format!("response exceeded the configured {maximum}-byte limit"),
        RetryClassification::Never,
    )
}

pub(in crate::client) fn service_error(
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

pub(in crate::client) fn protocol_error(error: ProtocolError) -> S3Error {
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
