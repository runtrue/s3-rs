//! Transport-independent AWS Signature Version 4 support.

mod canonical;
mod crypto;
mod request;

#[cfg(test)]
pub(crate) use canonical::canonical_uri;
pub(crate) use canonical::{
    CanonicalHeaders, Header, QueryParam, canonical_headers, canonical_query,
    canonical_uri_from_encoded,
};
pub(crate) use crypto::{derive_signing_key, payload_sha256_hex};
pub(crate) use request::{
    HeaderSigningOutput, HeaderSigningRequest, PresignedQuery, PresigningRequest,
    SigningCredentials, SigningError, SigningPath, SigningScope, sign_headers, sign_presigned,
};
