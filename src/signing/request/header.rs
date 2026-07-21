use time::OffsetDateTime;

use super::super::{Header, canonical_headers};
use super::common::{
    ALGORITHM, build_canonical_request, calculate_signature, credential_scope, format_timestamp,
    validate_common, validate_payload_hash,
};
use super::types::{
    HeaderSigningOutput, HeaderSigningRequest, SigningCredentials, SigningError, SigningScope,
};

/// Sign an HTTP request, returning the headers owned by the signing layer.
pub(crate) fn sign_headers(
    credentials: &SigningCredentials<'_>,
    scope: SigningScope<'_>,
    request: &HeaderSigningRequest<'_>,
    timestamp: OffsetDateTime,
) -> Result<HeaderSigningOutput, SigningError> {
    validate_common(credentials, scope, request.method)?;
    validate_payload_hash(request.payload_hash)?;
    reject_reserved_headers(request.headers)?;

    let (amz_date, date) = format_timestamp(timestamp)?;
    let mut headers = request.headers.to_vec();
    headers.push(Header::new("x-amz-date", &amz_date));
    if let Some(token) = credentials.session_token {
        headers.push(Header::new("x-amz-security-token", token));
    }
    let canonical_headers = canonical_headers(&headers)?;
    let canonical_request = build_canonical_request(
        request.method,
        request.uri_path,
        request.query,
        &canonical_headers,
        request.payload_hash,
    )?;
    let credential_scope = credential_scope(&date, scope);
    let signature = calculate_signature(
        credentials,
        scope,
        &date,
        &amz_date,
        &credential_scope,
        &canonical_request,
    )?;
    let authorization = format!(
        "{ALGORITHM} Credential={}/{credential_scope}, SignedHeaders={}, Signature={signature}",
        credentials.access_key_id,
        canonical_headers.signed()
    );

    Ok(HeaderSigningOutput {
        authorization,
        amz_date,
        security_token: credentials.session_token.map(str::to_owned),
    })
}

fn reject_reserved_headers(headers: &[Header<'_>]) -> Result<(), SigningError> {
    if headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("authorization")
            || header.name.eq_ignore_ascii_case("x-amz-date")
            || header.name.eq_ignore_ascii_case("x-amz-security-token")
    }) {
        return Err(SigningError::ReservedHeader);
    }
    Ok(())
}
