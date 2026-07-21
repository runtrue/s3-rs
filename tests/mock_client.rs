//! Deterministic wire-level client and fault-handling tests.

mod support;

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use http::StatusCode;
use s3_wire::{
    ByteStream, Credentials, DeleteObjectRequest, Endpoint, ErrorCategory, GetObjectRequest,
    ListObjectsV2Request, ManagedMultipartUploadRequest, ObjectKey, PutObjectRequest, RetryPolicy,
    S3Client, S3Config, S3Error, StaticCredentialsProvider, TimeoutPhase,
};
use sha2::{Digest as _, Sha256};

use support::{MockServer, Reply};

const MINIMUM_PART_SIZE: usize = 5 * 1024 * 1024;

fn key(value: &str) -> ObjectKey {
    ObjectKey::new(value).expect("valid test key")
}

fn client(server: &MockServer) -> S3Client {
    client_with(server, 1024 * 1024, Duration::from_millis(100))
}

fn client_with(server: &MockServer, xml_limit: usize, idle_timeout: Duration) -> S3Client {
    let credentials = Credentials::new(
        "TESTACCESSKEY",
        "test-secret-key",
        Some("test-session-token".to_owned()),
    )
    .expect("valid test credentials");
    let config = S3Config::builder()
        .endpoint(Endpoint::new(server.endpoint()).expect("valid mock endpoint"))
        .allow_http_for_local_testing()
        .bucket("test-bucket")
        .region("us-east-1")
        .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
        .retry_policy(
            RetryPolicy::new(
                3,
                Duration::from_millis(1),
                Duration::from_millis(1),
                Duration::from_secs(1),
            )
            .expect("valid retry policy"),
        )
        .connect_timeout(Duration::from_secs(1))
        .attempt_timeout(Duration::from_secs(1))
        .operation_timeout(Duration::from_secs(2))
        .idle_body_timeout(idle_timeout)
        .max_xml_response_size(xml_limit)
        .max_error_response_size(4096)
        .multipart_part_size(MINIMUM_PART_SIZE as u64)
        .multipart_threshold(MINIMUM_PART_SIZE as u64)
        .multipart_concurrency(2)
        .max_multipart_in_flight_bytes((2 * MINIMUM_PART_SIZE) as u64)
        .build()
        .expect("valid test config");
    S3Client::new(config).expect("construct test client")
}

fn create_multipart_reply(key: &str) -> Reply {
    Reply::xml(
        StatusCode::OK,
        format!(
            "<InitiateMultipartUploadResult><Bucket>test-bucket</Bucket><Key>{key}</Key><UploadId>upload-123</UploadId></InitiateMultipartUploadResult>"
        )
        .into_bytes(),
    )
}

fn upload_part_reply(part: &str) -> Reply {
    Reply::Full {
        status: StatusCode::OK,
        headers: vec![("etag".to_owned(), format!("\"part-{part}\""))],
        body: Vec::new(),
    }
}

fn complete_multipart_reply(key: &str) -> Reply {
    Reply::xml(
        StatusCode::OK,
        format!(
            "<CompleteMultipartUploadResult><Bucket>test-bucket</Bucket><Key>{key}</Key><ETag>\"complete\"</ETag></CompleteMultipartUploadResult>"
        )
        .into_bytes(),
    )
}

async fn wait_for_request<F>(server: &MockServer, predicate: F)
where
    F: Fn(&support::CapturedRequest) -> bool,
{
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if server.requests().await.iter().any(&predicate) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("mock server observed the expected request");
}

async fn collect_download(mut body: s3_wire::ResponseStream) -> Result<Vec<u8>, S3Error> {
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        bytes.extend_from_slice(&chunk?);
    }
    Ok(bytes)
}

fn sha256_base64(bytes: &[u8]) -> String {
    BASE64_STANDARD.encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn signs_requests_with_required_sigv4_headers() {
    let server = MockServer::start(|_, _| Reply::empty(StatusCode::NO_CONTENT)).await;

    client(&server)
        .delete_object(DeleteObjectRequest::new(key("signed/object")))
        .await
        .expect("delete succeeds");

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.target, "/test-bucket/signed/object");
    assert!(
        request
            .header("authorization")
            .is_some_and(|value| value.starts_with("AWS4-HMAC-SHA256 Credential=TESTACCESSKEY/"))
    );
    assert!(request.header("x-amz-date").is_some());
    assert!(request.header("x-amz-content-sha256").is_some());
    assert_eq!(
        request.header("x-amz-security-token"),
        Some("test-session-token")
    );
}

#[tokio::test]
async fn retries_throttling_responses_up_to_the_configured_attempt_count() {
    let server = MockServer::start(|attempt, _| {
        if attempt < 3 {
            Reply::xml(
                StatusCode::SERVICE_UNAVAILABLE,
                b"<Error><Code>SlowDown</Code><Message>Reduce request rate</Message></Error>"
                    .to_vec(),
            )
        } else {
            Reply::empty(StatusCode::NO_CONTENT)
        }
    })
    .await;

    client(&server)
        .delete_object(DeleteObjectRequest::new(key("retry-me")))
        .await
        .expect("third attempt succeeds");

    assert_eq!(server.request_count().await, 3);
}

#[tokio::test]
async fn does_not_retry_a_non_replayable_upload_body() {
    let server = MockServer::start(|_, _| {
        Reply::xml(
            StatusCode::SERVICE_UNAVAILABLE,
            b"<Error><Code>SlowDown</Code></Error>".to_vec(),
        )
    })
    .await;
    let bytes = Bytes::from_static(b"one-shot");
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let body = ByteStream::from_stream(
        stream::once(std::future::ready(Ok(bytes.clone()))),
        u64::try_from(bytes.len()).expect("body length fits u64"),
        digest,
    );

    let error = client(&server)
        .put_object(PutObjectRequest::new(key("one-shot"), body))
        .await
        .expect_err("503 is returned to the caller");

    assert_eq!(error.category(), ErrorCategory::Throttling);
    assert_eq!(server.request_count().await, 1);
    assert_eq!(server.requests().await[0].body, bytes);
}

#[tokio::test]
async fn rejects_oversized_and_malformed_listing_xml() {
    let oversized = MockServer::start(|_, _| Reply::xml(StatusCode::OK, vec![b'x'; 65])).await;
    let error = client_with(&oversized, 64, Duration::from_millis(100))
        .list_objects_v2(ListObjectsV2Request::default())
        .await
        .expect_err("oversized XML must fail");
    assert_eq!(error.category(), ErrorCategory::OversizedResponse);

    let malformed = MockServer::start(|_, _| {
        Reply::xml(StatusCode::OK, b"<ListBucketResult><broken>".to_vec())
    })
    .await;
    let error = client(&malformed)
        .list_objects_v2(ListObjectsV2Request::default())
        .await
        .expect_err("malformed XML must fail");
    assert_eq!(error.category(), ErrorCategory::InvalidResponse);
}

#[tokio::test]
async fn detects_truncated_downloads() {
    let server = MockServer::start(|_, _| Reply::Truncated {
        status: StatusCode::OK,
        declared_length: 5,
        body: b"abc".to_vec(),
    })
    .await;
    let mut output = client(&server)
        .get_object(GetObjectRequest::new(key("truncated")))
        .await
        .expect("response headers are valid");

    let mut observed_error = None;
    while let Some(item) = output.body.next().await {
        if let Err(error) = item {
            observed_error = Some(error);
            break;
        }
    }
    let error = observed_error.expect("truncation is surfaced by the stream");
    assert_eq!(error.category(), ErrorCategory::Integrity);
}

#[tokio::test]
async fn times_out_an_idle_download_body() {
    let server = MockServer::start(|_, _| Reply::Idle {
        status: StatusCode::OK,
        declared_length: 1,
    })
    .await;
    let mut output = client_with(&server, 1024, Duration::from_millis(25))
        .get_object(GetObjectRequest::new(key("idle")))
        .await
        .expect("response headers are valid");

    let error = tokio::time::timeout(Duration::from_secs(1), output.body.next())
        .await
        .expect("client timeout completes deterministically")
        .expect("timeout is returned as a stream item")
        .expect_err("idle body must fail");
    assert_eq!(error.category(), ErrorCategory::Timeout);
    assert_eq!(error.timeout_phase(), Some(TimeoutPhase::ResponseBody));
}

#[tokio::test]
async fn verifies_a_full_object_sha256_checksum_at_download_eof() {
    let expected = b"checksum protected body".to_vec();
    let response_body = expected.clone();
    let checksum = sha256_base64(&expected);
    let server = MockServer::start(move |_, _| Reply::Full {
        status: StatusCode::OK,
        headers: vec![
            ("x-amz-checksum-sha256".to_owned(), checksum.clone()),
            ("x-amz-checksum-type".to_owned(), "FULL_OBJECT".to_owned()),
        ],
        body: response_body.clone(),
    })
    .await;
    let output = client(&server)
        .get_object(GetObjectRequest::new(key("checksummed")))
        .await
        .expect("checksum response headers are valid");

    assert_eq!(
        collect_download(output.body)
            .await
            .expect("matching checksum succeeds"),
        expected
    );
}

#[tokio::test]
async fn reports_a_full_object_sha256_mismatch_at_download_eof() {
    let response_body = b"corrupted body".to_vec();
    let checksum = sha256_base64(b"expected body");
    let server = MockServer::start(move |_, _| Reply::Full {
        status: StatusCode::OK,
        headers: vec![("x-amz-checksum-sha256".to_owned(), checksum.clone())],
        body: response_body.clone(),
    })
    .await;
    let output = client(&server)
        .get_object(GetObjectRequest::new(key("checksum-mismatch")))
        .await
        .expect("checksum response headers are valid");

    let error = collect_download(output.body)
        .await
        .expect_err("mismatched checksum fails at EOF");
    assert_eq!(error.category(), ErrorCategory::Integrity);
    assert_eq!(
        error.retry_classification(),
        s3_wire::RetryClassification::Never
    );
}

#[tokio::test]
async fn rejects_repeated_pagination_tokens() {
    let body = b"<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>repeat</NextContinuationToken></ListBucketResult>".to_vec();
    let server = MockServer::start(move |_, _| Reply::xml(StatusCode::OK, body.clone())).await;

    let error = client(&server)
        .list_objects_v2_all(ListObjectsV2Request::default(), 5)
        .await
        .expect_err("repeated token must fail");

    assert_eq!(error.category(), ErrorCategory::InvalidResponse);
    assert_eq!(server.request_count().await, 2);
}

#[tokio::test]
async fn does_not_forward_credentials_to_a_custom_redirect_target() {
    let credential_sink = MockServer::start(|_, _| Reply::empty(StatusCode::NO_CONTENT)).await;
    let redirect_target = format!("{}/stolen", credential_sink.endpoint());
    let redirector = MockServer::start(move |_, _| Reply::Full {
        status: StatusCode::TEMPORARY_REDIRECT,
        headers: vec![
            ("location".to_owned(), redirect_target.clone()),
            ("x-amz-bucket-region".to_owned(), "us-west-2".to_owned()),
        ],
        body: Vec::new(),
    })
    .await;

    client(&redirector)
        .delete_object(DeleteObjectRequest::new(key("redirected")))
        .await
        .expect_err("custom redirect is not followed");

    assert_eq!(redirector.request_count().await, 1);
    assert_eq!(credential_sink.request_count().await, 0);
}

#[tokio::test]
async fn managed_multipart_upload_sends_exact_parts_then_ordered_completion() {
    let server =
        MockServer::start(
            |_, request| match (request.method.as_str(), request.target.as_str()) {
                ("POST", "/test-bucket/managed?uploads=") => create_multipart_reply("managed"),
                ("PUT", target) if target.contains("partNumber=1") => upload_part_reply("1"),
                ("PUT", target) if target.contains("partNumber=2") => upload_part_reply("2"),
                ("POST", "/test-bucket/managed?uploadId=upload-123") => {
                    complete_multipart_reply("managed")
                }
                _ => Reply::empty(StatusCode::NOT_FOUND),
            },
        )
        .await;
    let mut source = vec![b'a'; MINIMUM_PART_SIZE];
    source.extend_from_slice(b"tail");

    let output = client(&server)
        .multipart_upload(ManagedMultipartUploadRequest::from_bytes(
            key("managed"),
            source,
        ))
        .await
        .expect("managed multipart upload succeeds");

    assert_eq!(output.e_tag.as_deref(), Some("\"complete\""));
    let requests = server.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].target, "/test-bucket/managed?uploads=");
    assert_eq!(
        requests[3].target,
        "/test-bucket/managed?uploadId=upload-123"
    );
    let part_one = requests
        .iter()
        .find(|request| request.target.contains("partNumber=1"))
        .expect("part one request");
    let part_two = requests
        .iter()
        .find(|request| request.target.contains("partNumber=2"))
        .expect("part two request");
    assert_eq!(part_one.body, vec![b'a'; MINIMUM_PART_SIZE]);
    assert_eq!(part_two.body, b"tail");
    assert_eq!(
        requests[3].body,
        b"<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>\"part-1\"</ETag></Part><Part><PartNumber>2</PartNumber><ETag>\"part-2\"</ETag></Part></CompleteMultipartUpload>"
    );
}

#[tokio::test]
async fn managed_multipart_part_failure_aborts_without_completing() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("part-fails"),
        "PUT" if request.target.contains("partNumber=2") => Reply::xml(
            StatusCode::FORBIDDEN,
            b"<Error><Code>AccessDenied</Code><Message>part denied</Message></Error>".to_vec(),
        ),
        "PUT" => upload_part_reply("1"),
        "DELETE" => Reply::empty(StatusCode::NO_CONTENT),
        _ => Reply::empty(StatusCode::INTERNAL_SERVER_ERROR),
    })
    .await;
    let source = vec![b'x'; MINIMUM_PART_SIZE + 1];

    let error = client(&server)
        .multipart_upload(ManagedMultipartUploadRequest::from_bytes(
            key("part-fails"),
            source,
        ))
        .await
        .expect_err("selected part failure fails the upload");

    assert_eq!(error.category(), ErrorCategory::Authorization);
    let requests = server.requests().await;
    assert!(requests.iter().any(|request| request.method == "DELETE"));
    assert!(!requests.iter().any(|request| {
        request.method == "POST" && request.target.contains("uploadId=upload-123")
    }));
}

#[tokio::test]
async fn managed_multipart_preserves_primary_error_when_abort_fails() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("abort-fails"),
        "PUT" if request.target.contains("partNumber=2") => Reply::xml(
            StatusCode::FORBIDDEN,
            b"<Error><Code>AccessDenied</Code><Message>part denied</Message></Error>".to_vec(),
        ),
        "PUT" => upload_part_reply("1"),
        "DELETE" => Reply::xml(
            StatusCode::FORBIDDEN,
            b"<Error><Code>AccessDenied</Code><Message>abort denied</Message></Error>".to_vec(),
        ),
        _ => Reply::empty(StatusCode::INTERNAL_SERVER_ERROR),
    })
    .await;
    let source = vec![b'x'; MINIMUM_PART_SIZE + 1];

    let error = client(&server)
        .multipart_upload(ManagedMultipartUploadRequest::from_bytes(
            key("abort-fails"),
            source,
        ))
        .await
        .expect_err("part and cleanup both fail");

    assert_eq!(error.category(), ErrorCategory::Authorization);
    assert_eq!(
        error.cleanup_failure().map(s3_wire::S3Error::category),
        Some(ErrorCategory::Authorization)
    );
    assert_eq!(
        server
            .requests()
            .await
            .iter()
            .filter(|request| request.method == "DELETE")
            .count(),
        1
    );
}

#[tokio::test]
async fn cancelling_managed_multipart_eventually_aborts_the_upload() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("cancelled"),
        "PUT" => Reply::Stall,
        "DELETE" => Reply::empty(StatusCode::NO_CONTENT),
        _ => Reply::empty(StatusCode::INTERNAL_SERVER_ERROR),
    })
    .await;
    let upload_client = client(&server);
    let source = vec![b'x'; MINIMUM_PART_SIZE + 1];
    let upload = tokio::spawn(async move {
        upload_client
            .multipart_upload(ManagedMultipartUploadRequest::from_bytes(
                key("cancelled"),
                source,
            ))
            .await
    });
    wait_for_request(&server, |request| request.method == "PUT").await;

    upload.abort();
    upload.await.expect_err("caller task is cancelled");
    wait_for_request(&server, |request| request.method == "DELETE").await;

    let requests = server.requests().await;
    assert!(requests.iter().any(|request| request.method == "DELETE"));
    assert!(!requests.iter().any(|request| {
        request.method == "POST" && request.target.contains("uploadId=upload-123")
    }));
}
