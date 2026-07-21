use time::OffsetDateTime;

use super::super::{QueryParam, canonical_headers, canonical_query};
use super::common::{
    ALGORITHM, UNSIGNED_PAYLOAD, build_canonical_request, calculate_signature, credential_scope,
    format_timestamp, validate_common, validate_payload_hash,
};
use super::types::{
    PresignedQuery, PresigningRequest, SigningCredentials, SigningError, SigningScope,
};

const MAX_PRESIGN_EXPIRY: u64 = 7 * 24 * 60 * 60;

/// Create the complete encoded query string for a presigned GET or PUT.
pub(crate) fn sign_presigned(
    credentials: &SigningCredentials<'_>,
    scope: SigningScope<'_>,
    request: &PresigningRequest<'_>,
    timestamp: OffsetDateTime,
) -> Result<PresignedQuery, SigningError> {
    validate_common(credentials, scope, request.method)?;
    if !matches!(request.method, "GET" | "PUT") {
        return Err(SigningError::UnsupportedPresignMethod);
    }
    let expires = request.expires.as_secs();
    if expires == 0 || expires > MAX_PRESIGN_EXPIRY || request.expires.subsec_nanos() != 0 {
        return Err(SigningError::InvalidExpiry);
    }
    reject_reserved_query(request.query)?;
    if let Some(payload_hash) = request.payload_hash {
        validate_payload_hash(payload_hash)?;
    }
    let canonical_headers = canonical_headers(request.headers)?;
    let (amz_date, date) = format_timestamp(timestamp)?;
    let credential_scope = credential_scope(&date, scope);
    let credential = format!("{}/{credential_scope}", credentials.access_key_id);
    let expires = expires.to_string();

    let mut parameters = request.query.to_vec();
    parameters.extend([
        QueryParam::new("X-Amz-Algorithm", ALGORITHM),
        QueryParam::new("X-Amz-Credential", &credential),
        QueryParam::new("X-Amz-Date", &amz_date),
        QueryParam::new("X-Amz-Expires", &expires),
        QueryParam::new("X-Amz-SignedHeaders", canonical_headers.signed()),
    ]);
    if let Some(token) = credentials.session_token {
        parameters.push(QueryParam::new("X-Amz-Security-Token", token));
    }

    let canonical_request = build_canonical_request(
        request.method,
        request.uri_path,
        &parameters,
        &canonical_headers,
        request.payload_hash.unwrap_or(UNSIGNED_PAYLOAD),
    )?;
    let signature = calculate_signature(
        credentials,
        scope,
        &date,
        &amz_date,
        &credential_scope,
        &canonical_request,
    )?;
    let mut query = canonical_query(&parameters);
    query.push_str("&X-Amz-Signature=");
    query.push_str(&signature);
    Ok(PresignedQuery(query))
}

fn reject_reserved_query(query: &[QueryParam<'_>]) -> Result<(), SigningError> {
    if query
        .iter()
        .any(|parameter| parameter.name.eq_ignore_ascii_case("X-Amz-Signature"))
        || query.iter().any(|parameter| {
            matches!(
                parameter.name.to_ascii_lowercase().as_str(),
                "x-amz-algorithm"
                    | "x-amz-credential"
                    | "x-amz-date"
                    | "x-amz-expires"
                    | "x-amz-security-token"
                    | "x-amz-signedheaders"
            )
        })
    {
        return Err(SigningError::ReservedQueryParameter);
    }
    Ok(())
}
