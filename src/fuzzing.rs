//! Restricted validation harnesses for fuzzers and benchmarks.
//!
//! This module is intentionally hidden and absent from default builds. Its
//! functions expose deterministic transformations and coarse parse summaries,
//! never credentials, signatures, response bodies, or other bearer material.

use bytes::Bytes;
use http_body_util::BodyExt as _;
use time::{OffsetDateTime, macros::datetime};

use crate::operation::ObjectKey;
use crate::protocol::{
    CompleteMultipartResponse, CopyObjectResponse, parse_complete_multipart_upload,
    parse_copy_object, parse_create_multipart_upload, parse_list_multipart_uploads,
    parse_list_objects_v2, parse_s3_error,
};
use crate::signing::{
    Header, HeaderSigningRequest, QueryParam, SigningCredentials, SigningPath, SigningScope,
    canonical_headers, canonical_query, canonical_uri, sign_headers,
};
use crate::stream::ByteStream;

/// Opaque validation failure returned by restricted harness entry points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessError;

/// Canonicalizes a raw S3 request path.
pub fn canonical_uri_path(path: &str) -> String {
    canonical_uri(path)
}

/// Canonicalizes owned query name/value pairs.
pub fn canonical_query_pairs(parameters: &[(String, String)]) -> String {
    let borrowed = parameters
        .iter()
        .map(|(name, value)| QueryParam::new(name, value))
        .collect::<Vec<_>>();
    canonical_query(&borrowed)
}

/// Canonicalizes owned header name/value pairs.
pub fn canonical_header_pairs(
    headers: &[(String, String)],
) -> Result<(String, String), HarnessError> {
    let borrowed = headers
        .iter()
        .map(|(name, value)| Header::new(name, value))
        .collect::<Vec<_>>();
    canonical_headers(&borrowed)
        .map(|headers| (headers.canonical().to_owned(), headers.signed().to_owned()))
        .map_err(|_| HarnessError)
}

/// Parses an S3 error document and returns presence flags for bounded fields.
pub fn parse_error_xml(body: &[u8], maximum: usize) -> Result<[bool; 6], HarnessError> {
    parse_s3_error(body, maximum)
        .map(|error| {
            [
                error.code.is_some(),
                error.message.is_some(),
                error.request_id.is_some(),
                error.host_id.is_some(),
                error.resource.is_some(),
                error.region.is_some(),
            ]
        })
        .map_err(|_| HarnessError)
}

/// Parses a listing document and returns object, prefix, truncation, and token data.
pub fn parse_listing_xml(
    body: &[u8],
    maximum: usize,
) -> Result<(usize, usize, bool, Option<String>), HarnessError> {
    parse_list_objects_v2(body, maximum)
        .map(|output| {
            (
                output.objects.len(),
                output.common_prefixes.len(),
                output.is_truncated,
                output.next_continuation_token,
            )
        })
        .map_err(|_| HarnessError)
}

/// Exercises every multipart response parser and returns the number accepted.
pub fn parse_multipart_xml(body: &[u8], maximum: usize) -> usize {
    let created = usize::from(parse_create_multipart_upload(body, maximum).is_ok());
    let completed = usize::from(
        parse_complete_multipart_upload(body, maximum)
            .map(|result| match result {
                CompleteMultipartResponse::Complete(_) => 1,
                CompleteMultipartResponse::EmbeddedError(_) => 2,
            })
            .is_ok(),
    );
    let copied = usize::from(
        parse_copy_object(body, maximum)
            .map(|result| match result {
                CopyObjectResponse::Complete(_) => 1,
                CopyObjectResponse::EmbeddedError(_) => 2,
            })
            .is_ok(),
    );
    let listed = usize::from(parse_list_multipart_uploads(body, maximum).is_ok());
    created + completed + copied + listed
}

/// Performs one representative signing operation and returns only output size.
pub fn signing_output_size(path: &str) -> Result<usize, HarnessError> {
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let credentials = SigningCredentials::new("TESTACCESS", b"fixed-benchmark-key", None);
    let headers = [Header::new("host", "bucket.example.test")];
    let request = HeaderSigningRequest {
        method: "GET",
        uri_path: SigningPath::raw(path),
        query: &[],
        headers: &headers,
        payload_hash: EMPTY_SHA256,
    };
    sign_headers(
        &credentials,
        SigningScope::new("us-east-1", "s3"),
        &request,
        benchmark_timestamp(),
    )
    .map(|output| output.authorization().len())
    .map_err(|_| HarnessError)
}

/// Prepares and consumes one in-memory upload body, returning its byte count.
pub async fn consume_byte_stream(bytes: Bytes) -> Result<usize, HarnessError> {
    let prepared = ByteStream::from_bytes(bytes)
        .prepare()
        .await
        .map_err(|_| HarnessError)?;
    let body = prepared.request_body().await.map_err(|_| HarnessError)?;
    body.collect()
        .await
        .map(|collected| collected.to_bytes().len())
        .map_err(|_| HarnessError)
}

/// Validates an object key and returns its encoded length after endpoint construction.
pub fn endpoint_object_path_size(
    endpoint: &crate::Endpoint,
    bucket: &str,
    key: &str,
    style: crate::AddressingStyle,
) -> Result<usize, HarnessError> {
    let key = ObjectKey::new(key).map_err(|_| HarnessError)?;
    endpoint
        .object_url(bucket, Some(key.as_str()), style)
        .map(|url| url.path_and_query().len())
        .map_err(|_| HarnessError)
}

fn benchmark_timestamp() -> OffsetDateTime {
    datetime!(2024-01-01 00:00:00 UTC)
}
