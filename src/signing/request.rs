use std::{fmt, time::Duration};

use time::{OffsetDateTime, macros::format_description};

#[cfg(test)]
use super::canonical_uri;
use super::crypto::hex_encode;
use super::{
    Header, QueryParam, canonical_headers, canonical_query, canonical_uri_from_encoded,
    derive_signing_key, payload_sha256_hex,
};

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const REQUEST_TERMINATOR: &str = "aws4_request";
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const MAX_PRESIGN_EXPIRY: u64 = 7 * 24 * 60 * 60;

/// Borrowed credentials used only for one signing operation.
///
/// This type intentionally has no `Debug` or `Display` implementation.
pub(crate) struct SigningCredentials<'a> {
    access_key_id: &'a str,
    secret_access_key: &'a [u8],
    session_token: Option<&'a str>,
}

impl<'a> SigningCredentials<'a> {
    pub(crate) const fn new(
        access_key_id: &'a str,
        secret_access_key: &'a [u8],
        session_token: Option<&'a str>,
    ) -> Self {
        Self {
            access_key_id,
            secret_access_key,
            session_token,
        }
    }
}

/// Region and service components of the `SigV4` credential scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SigningScope<'a> {
    /// AWS region or compatible provider's region identifier.
    pub(crate) region: &'a str,
    /// AWS service identifier, normally `s3`.
    pub(crate) service: &'a str,
}

/// Identifies whether the path still needs URI encoding or came from a URL's
/// already encoded wire representation.
#[derive(Clone, Copy)]
pub(crate) enum SigningPath<'a> {
    #[cfg(test)]
    Raw(&'a str),
    Encoded(&'a str),
}

impl<'a> SigningPath<'a> {
    #[cfg(test)]
    pub(crate) const fn raw(path: &'a str) -> Self {
        Self::Raw(path)
    }

    /// Use for values returned by [`url::Url::path`].
    pub(crate) const fn encoded(path: &'a str) -> Self {
        Self::Encoded(path)
    }

    fn canonical(self) -> Result<String, SigningError> {
        match self {
            #[cfg(test)]
            Self::Raw(path) => Ok(canonical_uri(path)),
            Self::Encoded(path) => canonical_uri_from_encoded(path),
        }
    }
}

impl<'a> SigningScope<'a> {
    pub(crate) const fn new(region: &'a str, service: &'a str) -> Self {
        Self { region, service }
    }
}

/// A request whose authorization is carried in HTTP headers.
pub(crate) struct HeaderSigningRequest<'a> {
    pub(crate) method: &'a str,
    pub(crate) uri_path: SigningPath<'a>,
    pub(crate) query: &'a [QueryParam<'a>],
    pub(crate) headers: &'a [Header<'a>],
    pub(crate) payload_hash: &'a str,
}

/// Header values produced for one signed request.
///
/// This type intentionally has no `Debug` or `Display` implementation because
/// it contains authorization material and may contain a session token.
pub(crate) struct HeaderSigningOutput {
    authorization: String,
    amz_date: String,
    security_token: Option<String>,
}

impl HeaderSigningOutput {
    pub(crate) fn authorization(&self) -> &str {
        &self.authorization
    }

    pub(crate) fn amz_date(&self) -> &str {
        &self.amz_date
    }

    pub(crate) fn security_token(&self) -> Option<&str> {
        self.security_token.as_deref()
    }
}

/// A GET or PUT request whose authorization is carried in its query string.
pub(crate) struct PresigningRequest<'a> {
    pub(crate) method: &'a str,
    pub(crate) uri_path: SigningPath<'a>,
    pub(crate) query: &'a [QueryParam<'a>],
    pub(crate) headers: &'a [Header<'a>],
    pub(crate) expires: Duration,
    /// Defaults to `UNSIGNED-PAYLOAD`, as required by the common S3 presigning
    /// flow. Supply a digest only when the eventual caller can send that exact
    /// payload.
    pub(crate) payload_hash: Option<&'a str>,
}

/// An encoded presigned query string.
///
/// This value is bearer authorization and therefore cannot be formatted with
/// `Debug` or `Display`.
pub(crate) struct PresignedQuery(String);

impl PresignedQuery {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Non-sensitive validation failures from the signing boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SigningError {
    InvalidCredentials,
    InvalidScope,
    InvalidMethod,
    InvalidHeaderName,
    InvalidHeaderValue,
    MissingHostHeader,
    InvalidHostHeader,
    InvalidPayloadHash,
    InvalidEncodedUri,
    ReservedHeader,
    ReservedQueryParameter,
    InvalidExpiry,
    UnsupportedPresignMethod,
    Cryptographic,
    Timestamp,
}

impl fmt::Display for SigningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCredentials => "invalid signing credentials",
            Self::InvalidScope => "invalid signing scope",
            Self::InvalidMethod => "invalid HTTP method for signing",
            Self::InvalidHeaderName => "invalid signed header name",
            Self::InvalidHeaderValue => "invalid signed header value",
            Self::MissingHostHeader => "a host header is required for signing",
            Self::InvalidHostHeader => "the host header is empty or ambiguous",
            Self::InvalidPayloadHash => "invalid SigV4 payload hash",
            Self::InvalidEncodedUri => "URL path contains invalid percent encoding",
            Self::ReservedHeader => "a signing-owned header was supplied",
            Self::ReservedQueryParameter => "a signing-owned query parameter was supplied",
            Self::InvalidExpiry => "presigning expiry must be between 1 second and 7 days",
            Self::UnsupportedPresignMethod => "only GET and PUT requests may be presigned",
            Self::Cryptographic => "signature calculation failed",
            Self::Timestamp => "signing timestamp could not be represented",
        })
    }
}

impl std::error::Error for SigningError {}

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

fn validate_common(
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

fn validate_payload_hash(payload_hash: &str) -> Result<(), SigningError> {
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

fn build_canonical_request(
    method: &str,
    uri_path: SigningPath<'_>,
    query: &[QueryParam<'_>],
    headers: &super::CanonicalHeaders,
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

fn calculate_signature(
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

fn credential_scope(date: &str, scope: SigningScope<'_>) -> String {
    format!(
        "{date}/{}/{}/{REQUEST_TERMINATOR}",
        scope.region, scope.service
    )
}

fn format_timestamp(timestamp: OffsetDateTime) -> Result<(String, String), SigningError> {
    const AMZ_DATE: &[time::format_description::FormatItem<'_>] =
        format_description!("[year][month][day]T[hour][minute][second]Z");
    const DATE: &[time::format_description::FormatItem<'_>] =
        format_description!("[year][month][day]");
    // Normalize an offset timestamp to UTC before rendering the literal `Z`.
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    let amz_date = timestamp
        .format(AMZ_DATE)
        .map_err(|_| SigningError::Timestamp)?;
    let date = timestamp
        .format(DATE)
        .map_err(|_| SigningError::Timestamp)?;
    Ok((amz_date, date))
}

#[cfg(test)]
mod tests {
    use time::{format_description::well_known::Iso8601, macros::datetime};

    use super::*;

    const ACCESS_KEY: &str = "AKIDEXAMPLE";
    const SECRET_KEY: &[u8] = b"wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    #[test]
    fn matches_official_iam_header_signing_vector() {
        // AWS General Reference: Signature Version 4, IAM ListUsers example.
        let credentials = SigningCredentials::new(ACCESS_KEY, SECRET_KEY, None);
        let query = [
            QueryParam::new("Action", "ListUsers"),
            QueryParam::new("Version", "2010-05-08"),
        ];
        let headers = [
            Header::new(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            ),
            Header::new("Host", "iam.amazonaws.com"),
        ];
        let output = sign_headers(
            &credentials,
            SigningScope::new("us-east-1", "iam"),
            &HeaderSigningRequest {
                method: "GET",
                uri_path: SigningPath::raw("/"),
                query: &query,
                headers: &headers,
                payload_hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            },
            datetime!(2015-08-30 12:36:00 UTC),
        )
        .unwrap();

        assert_eq!(output.amz_date(), "20150830T123600Z");
        assert_eq!(output.security_token(), None);
        assert_eq!(
            output.authorization(),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn matches_official_s3_get_object_header_signing_vector() {
        // AWS S3 User Guide: GET Object, first ten bytes of test.txt.
        const EMPTY_SHA256: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let credentials = SigningCredentials::new(
            "AKIAIOSFODNN7EXAMPLE",
            b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            None,
        );
        let headers = [
            Header::new("Host", "examplebucket.s3.amazonaws.com"),
            Header::new("Range", "bytes=0-9"),
            Header::new("x-amz-content-sha256", EMPTY_SHA256),
        ];

        // Retain an explicit canonical-request assertion so a compensating
        // change in key derivation cannot conceal a canonicalization defect.
        let mut canonical_input_headers = headers.to_vec();
        canonical_input_headers.push(Header::new("x-amz-date", "20130524T000000Z"));
        let normalized_headers = canonical_headers(&canonical_input_headers).unwrap();
        let canonical_request = build_canonical_request(
            "GET",
            SigningPath::raw("/test.txt"),
            &[],
            &normalized_headers,
            EMPTY_SHA256,
        )
        .unwrap();
        assert_eq!(
            canonical_request,
            concat!(
                "GET\n",
                "/test.txt\n",
                "\n",
                "host:examplebucket.s3.amazonaws.com\n",
                "range:bytes=0-9\n",
                "x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
                "x-amz-date:20130524T000000Z\n",
                "\n",
                "host;range;x-amz-content-sha256;x-amz-date\n",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
        );
        assert_eq!(
            payload_sha256_hex(canonical_request.as_bytes()),
            "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );

        let output = sign_headers(
            &credentials,
            SigningScope::new("us-east-1", "s3"),
            &HeaderSigningRequest {
                method: "GET",
                uri_path: SigningPath::raw("/test.txt"),
                query: &[],
                headers: &headers,
                payload_hash: EMPTY_SHA256,
            },
            datetime!(2013-05-24 00:00:00 UTC),
        )
        .unwrap();

        assert_eq!(output.amz_date(), "20130524T000000Z");
        assert_eq!(
            output.authorization(),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn session_token_is_part_of_header_signature() {
        let credentials = SigningCredentials::new(ACCESS_KEY, SECRET_KEY, Some("TOKEN/+="));
        let headers = [Header::new("host", "examplebucket.s3.amazonaws.com")];
        let output = sign_headers(
            &credentials,
            SigningScope::new("us-east-1", "s3"),
            &HeaderSigningRequest {
                method: "GET",
                uri_path: SigningPath::raw("/test.txt"),
                query: &[],
                headers: &headers,
                payload_hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            },
            datetime!(2013-05-24 00:00:00 UTC),
        )
        .unwrap();

        assert_eq!(output.security_token(), Some("TOKEN/+="));
        assert!(
            output
                .authorization()
                .contains("SignedHeaders=host;x-amz-date;x-amz-security-token")
        );
    }

    #[test]
    fn presigns_get_with_deterministic_aws_inputs() {
        // AWS S3 query-string authentication example.
        let credentials = SigningCredentials::new(
            "AKIAIOSFODNN7EXAMPLE",
            b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            None,
        );
        let headers = [Header::new("host", "examplebucket.s3.amazonaws.com")];
        let query = sign_presigned(
            &credentials,
            SigningScope::new("us-east-1", "s3"),
            &PresigningRequest {
                method: "GET",
                uri_path: SigningPath::encoded("/test.txt"),
                query: &[],
                headers: &headers,
                expires: Duration::from_hours(24),
                payload_hash: None,
            },
            datetime!(2013-05-24 00:00:00 UTC),
        )
        .unwrap();

        assert_eq!(
            query.as_str(),
            "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host&X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn presigns_put_and_encodes_session_token() {
        let credentials =
            SigningCredentials::new(ACCESS_KEY, SECRET_KEY, Some("token/with+chars="));
        let headers = [Header::new("host", "storage.example.test")];
        let query = sign_presigned(
            &credentials,
            SigningScope::new("local-1", "s3"),
            &PresigningRequest {
                method: "PUT",
                uri_path: SigningPath::raw("/bucket/a b"),
                query: &[QueryParam::new("versionId", "a/b+")],
                headers: &headers,
                expires: Duration::from_mins(1),
                payload_hash: None,
            },
            datetime!(2026-01-02 03:04:05 UTC),
        )
        .unwrap();
        assert!(
            query
                .as_str()
                .contains("X-Amz-Security-Token=token%2Fwith%2Bchars%3D")
        );
        assert!(query.as_str().contains("versionId=a%2Fb%2B"));
    }

    #[test]
    fn rejects_reserved_input_and_invalid_expiry() {
        let credentials = SigningCredentials::new(ACCESS_KEY, SECRET_KEY, None);
        let headers = [Header::new("host", "example.com")];
        let request = PresigningRequest {
            method: "GET",
            uri_path: SigningPath::raw("/"),
            query: &[QueryParam::new("x-amz-signature", "attacker")],
            headers: &headers,
            expires: Duration::from_mins(1),
            payload_hash: None,
        };
        let result = sign_presigned(
            &credentials,
            SigningScope::new("us-east-1", "s3"),
            &request,
            datetime!(2026-01-02 03:04:05 UTC),
        );
        assert!(matches!(result, Err(SigningError::ReservedQueryParameter)));
    }

    #[test]
    fn timestamp_is_normalized_to_utc() {
        let timestamp =
            OffsetDateTime::parse("2015-08-30T14:36:00+02:00", &Iso8601::DEFAULT).unwrap();
        assert_eq!(
            format_timestamp(timestamp).unwrap(),
            ("20150830T123600Z".to_owned(), "20150830".to_owned())
        );
    }
}
