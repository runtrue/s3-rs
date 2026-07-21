use http::{HeaderMap, StatusCode};

use super::headers::request_ids;
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::protocol::ParsedS3Error;

pub(super) fn embedded_complete_error(headers: &HeaderMap, parsed: ParsedS3Error) -> S3Error {
    let category = match parsed.code.as_deref() {
        Some("AccessDenied" | "AllAccessDisabled") => ErrorCategory::Authorization,
        Some("NoSuchBucket" | "NoSuchUpload" | "NotFound") => ErrorCategory::NotFound,
        Some("SlowDown" | "Throttling" | "ThrottlingException") => ErrorCategory::Throttling,
        Some("InternalError" | "ServiceUnavailable") => ErrorCategory::Server,
        _ => ErrorCategory::InvalidResponse,
    };
    let retry = match category {
        ErrorCategory::Throttling => RetryClassification::Throttled,
        ErrorCategory::Server => RetryClassification::Retryable,
        _ => RetryClassification::Never,
    };
    let message = parsed
        .message
        .as_deref()
        .and_then(safe_service_message)
        .unwrap_or_else(|| "S3 returned an embedded completion error".to_owned());
    let ids = request_ids(headers);
    S3Error::new(category, message, retry).with_service_details(
        parsed.code,
        StatusCode::OK,
        ids.request_id.or(parsed.request_id),
        ids.host_id.or(parsed.host_id),
    )
}

fn safe_service_message(message: &str) -> Option<String> {
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
