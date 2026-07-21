mod common;
mod header;
mod presign;
mod types;

#[cfg(test)]
use common::{build_canonical_request, format_timestamp};
pub(crate) use header::sign_headers;
pub(crate) use presign::sign_presigned;
pub(crate) use types::{
    HeaderSigningOutput, HeaderSigningRequest, PresignedQuery, PresigningRequest,
    SigningCredentials, SigningError, SigningPath, SigningScope,
};

#[cfg(test)]
mod tests;
