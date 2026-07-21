use time::{OffsetDateTime, macros::format_description};

use super::super::crypto::hex_encode;
use super::super::{
    CanonicalHeaders, QueryParam, canonical_query, derive_signing_key, payload_sha256_hex,
};
use super::types::{SigningCredentials, SigningError, SigningPath, SigningScope};

pub(super) const ALGORITHM: &str = "AWS4-HMAC-SHA256";
pub(super) const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const REQUEST_TERMINATOR: &str = "aws4_request";

pub(super) fn validate_common(
    credentials: &SigningCredentials<'_>,
    scope: SigningScope<'_>,
    method: &str,
) -> Result<(), SigningError> {
    if credentials.secret_access_key.is_empty()
        || credentials.access_key_id.is_empty()
        || !credentials
            .access_key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(SigningError::InvalidCredentials);
    }
    if credentials.session_token.is_some_and(str::is_empty) {
        return Err(SigningError::InvalidCredentials);
    }
    if !valid_scope_component(scope.region) || !valid_scope_component(scope.service) {
        return Err(SigningError::InvalidScope);
    }
    if method.is_empty()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(SigningError::InvalidMethod);
    }
    Ok(())
}

pub(super) fn validate_payload_hash(payload_hash: &str) -> Result<(), SigningError> {
    let is_sha256 = payload_hash.len() == 64
        && payload_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    let is_supported_literal = matches!(
        payload_hash,
        UNSIGNED_PAYLOAD
            | "STREAMING-AWS4-HMAC-SHA256-PAYLOAD"
            | "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER"
            | "STREAMING-UNSIGNED-PAYLOAD-TRAILER"
    );
    if is_sha256 || is_supported_literal {
        Ok(())
    } else {
        Err(SigningError::InvalidPayloadHash)
    }
}

fn valid_scope_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn build_canonical_request(
    method: &str,
    uri_path: SigningPath<'_>,
    query: &[QueryParam<'_>],
    headers: &CanonicalHeaders,
    payload_hash: &str,
) -> Result<String, SigningError> {
    Ok(format!(
        "{method}\n{}\n{}\n{}\n{}\n{payload_hash}",
        uri_path.canonical()?,
        canonical_query(query),
        headers.canonical(),
        headers.signed()
    ))
}

pub(super) fn calculate_signature(
    credentials: &SigningCredentials<'_>,
    scope: SigningScope<'_>,
    date: &str,
    amz_date: &str,
    credential_scope: &str,
    canonical_request: &str,
) -> Result<String, SigningError> {
    let request_hash = payload_sha256_hex(canonical_request.as_bytes());
    let string_to_sign = format!("{ALGORITHM}\n{amz_date}\n{credential_scope}\n{request_hash}");
    let key = derive_signing_key(
        credentials.secret_access_key,
        date,
        scope.region,
        scope.service,
    )?;
    Ok(hex_encode(&key.sign(string_to_sign.as_bytes())?))
}

pub(super) fn credential_scope(date: &str, scope: SigningScope<'_>) -> String {
    format!(
        "{date}/{}/{}/{REQUEST_TERMINATOR}",
        scope.region, scope.service
    )
}

pub(super) fn format_timestamp(
    timestamp: OffsetDateTime,
) -> Result<(String, String), SigningError> {
    const AMZ_DATE: &[time::format_description::FormatItem<'_>] =
        format_description!("[year][month][day]T[hour][minute][second]Z");
    const DATE: &[time::format_description::FormatItem<'_>] =
        format_description!("[year][month][day]");
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    let amz_date = timestamp
        .format(AMZ_DATE)
        .map_err(|_| SigningError::Timestamp)?;
    let date = timestamp
        .format(DATE)
        .map_err(|_| SigningError::Timestamp)?;
    Ok((amz_date, date))
}
