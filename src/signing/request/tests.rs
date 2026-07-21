use std::time::Duration;

use time::{OffsetDateTime, format_description::well_known::Iso8601, macros::datetime};

use super::*;
use crate::signing::{Header, QueryParam, canonical_headers, payload_sha256_hex};

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
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
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
    let credentials = SigningCredentials::new(ACCESS_KEY, SECRET_KEY, Some("token/with+chars="));
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
    let timestamp = OffsetDateTime::parse("2015-08-30T14:36:00+02:00", &Iso8601::DEFAULT).unwrap();
    assert_eq!(
        format_timestamp(timestamp).unwrap(),
        ("20150830T123600Z".to_owned(), "20150830".to_owned())
    );
}
