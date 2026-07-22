use std::{fmt, time::Duration};

#[cfg(any(test, feature = "fuzzing"))]
use super::super::canonical_uri;
use super::super::{Header, QueryParam, canonical_uri_from_encoded};

/// Borrowed credentials used only for one signing operation.
///
/// This type intentionally has no `Debug` or `Display` implementation.
pub(crate) struct SigningCredentials<'a> {
    pub(super) access_key_id: &'a str,
    pub(super) secret_access_key: &'a [u8],
    pub(super) session_token: Option<&'a str>,
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

impl<'a> SigningScope<'a> {
    pub(crate) const fn new(region: &'a str, service: &'a str) -> Self {
        Self { region, service }
    }
}

/// Identifies whether the path still needs URI encoding or came from a URL's
/// already encoded wire representation.
#[derive(Clone, Copy)]
pub(crate) enum SigningPath<'a> {
    #[cfg(any(test, feature = "fuzzing"))]
    Raw(&'a str),
    Encoded(&'a str),
}

impl<'a> SigningPath<'a> {
    #[cfg(any(test, feature = "fuzzing"))]
    pub(crate) const fn raw(path: &'a str) -> Self {
        Self::Raw(path)
    }

    /// Use for values taken from an already encoded URI path.
    pub(crate) const fn encoded(path: &'a str) -> Self {
        Self::Encoded(path)
    }

    pub(super) fn canonical(self) -> Result<String, SigningError> {
        match self {
            #[cfg(any(test, feature = "fuzzing"))]
            Self::Raw(path) => Ok(canonical_uri(path)),
            Self::Encoded(path) => canonical_uri_from_encoded(path),
        }
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
    pub(super) authorization: String,
    pub(super) amz_date: String,
    pub(super) security_token: Option<String>,
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

/// An S3 request whose authorization is carried in its query string.
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
pub(crate) struct PresignedQuery(pub(super) String);

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
            Self::UnsupportedPresignMethod => "unsupported HTTP method for S3 presigning",
            Self::Cryptographic => "signature calculation failed",
            Self::Timestamp => "signing timestamp could not be represented",
        })
    }
}

impl std::error::Error for SigningError {}
