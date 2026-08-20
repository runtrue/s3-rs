//! Deterministic wire-level client and fault-handling tests.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use http::{HeaderMap, StatusCode};
use s3_wire::{
    AbortMultipartUploadRequest, ByteStream, ChecksumAlgorithm, CompleteMultipartUploadRequest,
    CompletedPart, Conditions, CopyMetadataDirective, CopyObjectRequest, CopyPartRange, CopySource,
    CreateMultipartUploadRequest, Credentials, DeleteObjectIdentifier, DeleteObjectRequest,
    DeleteObjectsRequest, Endpoint, ErrorCategory, GetObjectRequest, HeadObjectRequest,
    ListMultipartUploadsRequest, ListObjectsV2Request, ListPartsRequest, ManagedMultipartHeaders,
    ManagedMultipartUploadRequest, MultipartOptions, ObjectKey, PartNumber, PutObjectRequest,
    RequestEvent, RequestEventKind, RequestObserver, RetryPolicy, RetryStopReason, S3Client,
    S3Config, S3Error, StaticCredentialsProvider, TimeoutPhase, UploadId, UploadPartCopyRequest,
    UploadPartRequest,
};
use sha2::{Digest as _, Sha256};

use support::{MockServer, Reply};

const MINIMUM_PART_SIZE: usize = 5 * 1024 * 1024;

#[tokio::test]
async fn custom_headers_are_signed_and_preserved_across_retries() {
    let server = MockServer::start(|attempt, _| {
        if attempt == 1 {
            Reply::empty(StatusCode::INTERNAL_SERVER_ERROR)
        } else {
            Reply::empty(StatusCode::OK)
        }
    })
    .await;
    let mut headers = HeaderMap::new();
    headers.append("x-example", "one".parse().unwrap());
    headers.append("x-example", "two".parse().unwrap());
    headers.insert("user-agent", "custom-agent".parse().unwrap());
    let request = PutObjectRequest::new(key("custom-headers"), ByteStream::from_bytes("body"))
        .with_headers(headers);

    client(&server).put_object(request).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.header_values("x-example"), ["one", "two"]);
        assert_eq!(request.header("user-agent"), Some("custom-agent"));
        assert!(
            request
                .header("authorization")
                .is_some_and(|value| value.contains("x-example") && value.contains("user-agent"))
        );
    }
}

#[tokio::test]
async fn every_primitive_operation_sends_and_signs_custom_headers() {
    let server = MockServer::start(|attempt, _| match attempt {
        5 => Reply::xml(StatusCode::OK, b"<DeleteResult/>".to_vec()),
        6 => Reply::xml(
            StatusCode::OK,
            b"<CopyObjectResult><ETag>&quot;copy&quot;</ETag></CopyObjectResult>".to_vec(),
        ),
        7 => Reply::xml(
            StatusCode::OK,
            b"<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>false</IsTruncated></ListBucketResult>".to_vec(),
        ),
        8 => Reply::xml(
            StatusCode::OK,
            b"<InitiateMultipartUploadResult><Key>multipart</Key><UploadId>upload</UploadId></InitiateMultipartUploadResult>".to_vec(),
        ),
        9 => Reply::Full {
            status: StatusCode::OK,
            headers: vec![("etag".to_owned(), "\"part\"".to_owned())],
            body: Vec::new(),
        },
        10 => Reply::xml(
            StatusCode::OK,
            b"<CopyPartResult><ETag>&quot;part-copy&quot;</ETag></CopyPartResult>".to_vec(),
        ),
        11 => Reply::xml(
            StatusCode::OK,
            b"<CompleteMultipartUploadResult><Key>multipart</Key></CompleteMultipartUploadResult>".to_vec(),
        ),
        13 => Reply::xml(
            StatusCode::OK,
            b"<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>".to_vec(),
        ),
        14 => Reply::xml(
            StatusCode::OK,
            b"<ListPartsResult><IsTruncated>false</IsTruncated></ListPartsResult>".to_vec(),
        ),
        _ => Reply::empty(StatusCode::OK),
    })
    .await;
    let client = client(&server);
    let custom = |headers: &mut HeaderMap, operation: &'static str| {
        headers.insert("x-operation", operation.parse().unwrap());
    };

    let mut put = PutObjectRequest::new(key("put"), ByteStream::from_bytes("body"));
    custom(&mut put.headers, "put");
    client.put_object(put).await.unwrap();
    let mut get = GetObjectRequest::new(key("get"));
    custom(&mut get.headers, "get");
    client.get_object(get).await.unwrap();
    let mut head = HeadObjectRequest::new(key("head"));
    custom(&mut head.headers, "head");
    client.head_object(head).await.unwrap();
    let mut delete = DeleteObjectRequest::new(key("delete"));
    custom(&mut delete.headers, "delete");
    client.delete_object(delete).await.unwrap();
    let mut deletes =
        DeleteObjectsRequest::new(vec![DeleteObjectIdentifier::new(key("batch"))]).unwrap();
    custom(&mut deletes.headers, "delete-objects");
    client.delete_objects(deletes).await.unwrap();
    let source = CopySource {
        bucket: "source".to_owned(),
        key: key("source"),
        version_id: None,
    };
    let mut copy = CopyObjectRequest::new(source.clone(), key("copy"));
    custom(&mut copy.headers, "copy");
    client.copy_object(copy).await.unwrap();
    let mut list = ListObjectsV2Request::default();
    custom(&mut list.headers, "list-objects");
    client.list_objects_v2(list).await.unwrap();
    let upload_id = UploadId::new("upload").unwrap();
    let mut create = CreateMultipartUploadRequest::new(key("multipart"));
    custom(&mut create.headers, "create");
    client.create_multipart_upload(create).await.unwrap();
    let mut part = UploadPartRequest::new(
        key("multipart"),
        upload_id.clone(),
        PartNumber::new(1).unwrap(),
        ByteStream::from_bytes("part"),
    );
    custom(&mut part.headers, "upload-part");
    client.upload_part(part).await.unwrap();
    let mut part_copy = UploadPartCopyRequest::new(
        key("multipart"),
        upload_id.clone(),
        PartNumber::new(2).unwrap(),
        source,
    );
    custom(&mut part_copy.headers, "upload-part-copy");
    client.upload_part_copy(part_copy).await.unwrap();
    let mut complete = CompleteMultipartUploadRequest::new(
        key("multipart"),
        upload_id.clone(),
        vec![CompletedPart::new(1, "\"part\"").unwrap()],
    )
    .unwrap();
    custom(&mut complete.headers, "complete");
    client.complete_multipart_upload(complete).await.unwrap();
    let mut abort = AbortMultipartUploadRequest::new(key("multipart"), upload_id.clone());
    custom(&mut abort.headers, "abort");
    client.abort_multipart_upload(abort).await.unwrap();
    let mut uploads = ListMultipartUploadsRequest::default();
    custom(&mut uploads.headers, "list-uploads");
    client.list_multipart_uploads(uploads).await.unwrap();
    let mut parts = ListPartsRequest::new(key("multipart"), upload_id);
    custom(&mut parts.headers, "list-parts");
    client.list_parts(parts).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 14);
    for request in requests {
        assert!(request.header("x-operation").is_some());
        assert!(
            request
                .header("authorization")
                .is_some_and(|value| value.contains("x-operation"))
        );
    }
}

#[tokio::test]
async fn managed_multipart_headers_are_isolated_by_phase() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => Reply::xml(
            StatusCode::OK,
            b"<InitiateMultipartUploadResult><Key>managed</Key><UploadId>upload</UploadId></InitiateMultipartUploadResult>".to_vec(),
        ),
        "PUT" => Reply::Full {
            status: StatusCode::OK,
            headers: vec![("etag".to_owned(), "\"part\"".to_owned())],
            body: Vec::new(),
        },
        "POST" => Reply::xml(
            StatusCode::OK,
            b"<CompleteMultipartUploadResult><Key>managed</Key></CompleteMultipartUploadResult>".to_vec(),
        ),
        _ => unreachable!(),
    })
    .await;
    let mut source = vec![0_u8; MINIMUM_PART_SIZE];
    source.push(0);
    let mut headers = ManagedMultipartHeaders::default();
    headers
        .create
        .insert("x-phase-create", "yes".parse().unwrap());
    headers
        .upload_part
        .insert("x-phase-part", "yes".parse().unwrap());
    headers
        .complete
        .insert("x-phase-complete", "yes".parse().unwrap());
    headers
        .abort
        .insert("x-phase-abort", "yes".parse().unwrap());
    let request = ManagedMultipartUploadRequest::from_bytes(key("managed"), Bytes::from(source))
        .with_options(
            MultipartOptions::new(MINIMUM_PART_SIZE as u64, 2)
                .expect("valid test multipart options"),
        )
        .with_headers(headers);

    client(&server).multipart_upload(request).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 4);
    let create = requests
        .iter()
        .find(|request| request.method == "POST" && request.target.ends_with("?uploads="))
        .unwrap();
    let complete = requests
        .iter()
        .find(|request| request.method == "POST" && request.target.contains("uploadId="))
        .unwrap();
    assert_eq!(create.header("x-phase-create"), Some("yes"));
    assert_eq!(complete.header("x-phase-complete"), Some("yes"));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "PUT")
            .count(),
        2
    );
    for request in &requests {
        let expected = match request.method.as_str() {
            "PUT" => "x-phase-part",
            "POST" if request.target.ends_with("?uploads=") => "x-phase-create",
            "POST" => "x-phase-complete",
            _ => unreachable!(),
        };
        for name in [
            "x-phase-create",
            "x-phase-part",
            "x-phase-complete",
            "x-phase-abort",
        ] {
            assert_eq!(request.header(name).is_some(), name == expected);
        }
    }
}

#[tokio::test]
async fn managed_multipart_abort_uses_cleanup_headers_after_part_failure() {
    let server = MockServer::start(|attempt, _| match attempt {
        1 => Reply::xml(
            StatusCode::OK,
            b"<InitiateMultipartUploadResult><Key>managed</Key><UploadId>upload</UploadId></InitiateMultipartUploadResult>".to_vec(),
        ),
        2 => Reply::xml(
            StatusCode::FORBIDDEN,
            b"<Error><Code>AccessDenied</Code></Error>".to_vec(),
        ),
        3 => Reply::empty(StatusCode::NO_CONTENT),
        _ => unreachable!(),
    })
    .await;
    let mut request = ManagedMultipartUploadRequest::from_bytes(
        key("managed"),
        Bytes::from(vec![0_u8; MINIMUM_PART_SIZE]),
    );
    request
        .headers
        .create
        .insert("x-phase-create", "yes".parse().unwrap());
    request
        .headers
        .upload_part
        .insert("x-phase-part", "yes".parse().unwrap());
    request
        .headers
        .abort
        .insert("x-phase-abort", "yes".parse().unwrap());

    client(&server).multipart_upload(request).await.unwrap_err();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].header("x-phase-abort"), Some("yes"));
    assert!(requests[2].header("x-phase-create").is_none());
    assert!(requests[2].header("x-phase-part").is_none());
}

#[tokio::test]
async fn managed_multipart_abort_uses_cleanup_headers_after_part_preparation_failure() {
    let server = MockServer::start(|attempt, _| match attempt {
        1 => Reply::xml(
            StatusCode::OK,
            b"<InitiateMultipartUploadResult><Key>managed</Key><UploadId>upload</UploadId></InitiateMultipartUploadResult>".to_vec(),
        ),
        2 => Reply::empty(StatusCode::NO_CONTENT),
        _ => unreachable!(),
    })
    .await;
    let mut request = ManagedMultipartUploadRequest::from_bytes(
        key("managed"),
        Bytes::from(vec![0_u8; MINIMUM_PART_SIZE]),
    )
    .with_checksum_algorithm(ChecksumAlgorithm::Crc32c)
    .unwrap();
    request.headers.upload_part.insert(
        "x-amz-checksum-crc32c",
        "should-not-appear".parse().unwrap(),
    );
    request
        .headers
        .abort
        .insert("x-phase-abort", "yes".parse().unwrap());

    let error = client(&server).multipart_upload(request).await.unwrap_err();

    assert!(error.message().contains("x-amz-checksum-crc32c"));
    assert!(!error.message().contains("should-not-appear"));
    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "DELETE");
    assert_eq!(requests[1].header("x-phase-abort"), Some("yes"));
    assert!(requests[1].header("x-amz-checksum-crc32c").is_none());
}

#[tokio::test]
async fn custom_header_collisions_fail_before_network_io() {
    let server = MockServer::start(|_, _| Reply::empty(StatusCode::OK)).await;
    let mut request = PutObjectRequest::new(key("collision"), ByteStream::from_bytes("body"));
    request
        .headers
        .insert("content-type", "secret".parse().unwrap());
    request.content_type = Some("text/plain".to_owned());

    let error = client(&server).put_object(request).await.unwrap_err();
    assert!(error.message().contains("content-type"));
    assert!(!error.message().contains("secret"));

    let mut managed = ManagedMultipartUploadRequest::from_bytes(
        key("managed-collision"),
        Bytes::from(vec![0_u8; MINIMUM_PART_SIZE]),
    )
    .with_content_type("text/plain");
    managed
        .headers
        .create
        .insert("content-type", "managed-secret".parse().unwrap());
    let error = client(&server).multipart_upload(managed).await.unwrap_err();
    assert!(error.message().contains("content-type"));
    assert!(!error.message().contains("managed-secret"));
    assert_eq!(server.request_count().await, 0);
}

fn key(value: &str) -> ObjectKey {
    ObjectKey::new(value).expect("valid test key")
}

fn client(server: &MockServer) -> S3Client {
    client_with(server, 1024 * 1024, Duration::from_millis(100))
}

fn client_with(server: &MockServer, xml_limit: usize, idle_timeout: Duration) -> S3Client {
    client_with_observer(server, xml_limit, idle_timeout, None)
}

fn client_with_observer(
    server: &MockServer,
    xml_limit: usize,
    idle_timeout: Duration,
    observer: Option<Arc<dyn RequestObserver>>,
) -> S3Client {
    let credentials = Credentials::new(
        "TESTACCESSKEY",
        "test-secret-key",
        Some("test-session-token".to_owned()),
    )
    .expect("valid test credentials");
    let mut builder = S3Config::builder()
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
        .max_error_response_size(4096);
    if let Some(observer) = observer {
        builder = builder.observer(observer);
    }
    let config = builder.build().expect("valid test config");
    S3Client::new(config).expect("construct test client")
}

#[derive(Default)]
struct EventRecorder {
    events: Mutex<Vec<RequestEventKind>>,
}

impl RequestObserver for EventRecorder {
    fn on_event(&self, event: RequestEvent<'_>) {
        self.events.lock().unwrap().push(event.kind);
    }
}

fn managed_bytes(key: ObjectKey, bytes: Vec<u8>) -> ManagedMultipartUploadRequest {
    ManagedMultipartUploadRequest::from_bytes(key, bytes).with_options(
        MultipartOptions::new(MINIMUM_PART_SIZE as u64, 2).expect("valid test multipart options"),
    )
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
async fn observer_records_complete_retry_lifecycle() {
    let server = MockServer::start(|attempt, _| {
        if attempt == 1 {
            Reply::xml(
                StatusCode::SERVICE_UNAVAILABLE,
                b"<Error><Code>SlowDown</Code></Error>".to_vec(),
            )
        } else {
            Reply::empty(StatusCode::NO_CONTENT)
        }
    })
    .await;
    let recorder = Arc::new(EventRecorder::default());
    let observer: Arc<dyn RequestObserver> = recorder.clone();

    client_with_observer(
        &server,
        1024 * 1024,
        Duration::from_millis(100),
        Some(observer),
    )
    .delete_object(DeleteObjectRequest::new(key("observed")))
    .await
    .expect("retry succeeds");

    assert_eq!(
        *recorder.events.lock().unwrap(),
        vec![
            RequestEventKind::AttemptStarted,
            RequestEventKind::RetryScheduled,
            RequestEventKind::AttemptStarted,
            RequestEventKind::AttemptSucceeded,
        ]
    );
}

#[tokio::test]
async fn retries_embedded_copy_errors_returned_with_http_200() {
    let server = MockServer::start(|attempt, _| {
        if attempt == 1 {
            Reply::xml(
                StatusCode::OK,
                b"<Error><Code>SlowDown</Code><Message>retry later</Message></Error>".to_vec(),
            )
        } else {
            Reply::xml(
                StatusCode::OK,
                b"<CopyObjectResult><ETag>\"copied\"</ETag></CopyObjectResult>".to_vec(),
            )
        }
    })
    .await;

    let output = client(&server)
        .copy_object(CopyObjectRequest {
            source: CopySource {
                bucket: "source-bucket".to_owned(),
                key: key("source"),
                version_id: None,
            },
            destination: key("destination"),
            source_conditions: Conditions::default(),
            metadata: CopyMetadataDirective::Copy,
            headers: HeaderMap::new(),
        })
        .await
        .expect("retryable embedded error is retried");

    assert_eq!(output.e_tag.as_deref(), Some("\"copied\""));
    assert_eq!(server.request_count().await, 2);
}

#[tokio::test]
async fn retries_interrupted_collected_copy_responses() {
    let server = MockServer::start(|attempt, _| {
        if attempt == 1 {
            Reply::Truncated {
                status: StatusCode::OK,
                declared_length: 100,
                body: b"<CopyObjectResult>".to_vec(),
            }
        } else {
            Reply::xml(
                StatusCode::OK,
                b"<CopyObjectResult><ETag>&quot;copied&quot;</ETag></CopyObjectResult>".to_vec(),
            )
        }
    })
    .await;

    let output = client(&server)
        .copy_object(CopyObjectRequest {
            source: CopySource {
                bucket: "source-bucket".to_owned(),
                key: key("source"),
                version_id: None,
            },
            destination: key("destination"),
            source_conditions: Conditions::default(),
            metadata: CopyMetadataDirective::Copy,
            headers: HeaderMap::new(),
        })
        .await
        .expect("interrupted collected body is retried");

    assert_eq!(output.e_tag.as_deref(), Some("\"copied\""));
    assert_eq!(server.request_count().await, 2);
}

#[tokio::test]
async fn does_not_retry_permanent_embedded_copy_errors() {
    let server = MockServer::start(|_, _| {
        Reply::xml(
            StatusCode::OK,
            b"<Error><Code>AccessDenied</Code><Message>copy denied</Message></Error>".to_vec(),
        )
    })
    .await;

    let error = client(&server)
        .copy_object(CopyObjectRequest {
            source: CopySource {
                bucket: "source-bucket".to_owned(),
                key: key("source"),
                version_id: None,
            },
            destination: key("destination"),
            source_conditions: Conditions::default(),
            metadata: CopyMetadataDirective::Copy,
            headers: HeaderMap::new(),
        })
        .await
        .expect_err("permanent embedded error fails the copy");

    assert_eq!(error.category(), ErrorCategory::Authorization);
    assert_eq!(error.attempts(), 1);
    assert_eq!(
        error.retry_stop_reason(),
        Some(RetryStopReason::NonRetryable)
    );
    assert_eq!(server.request_count().await, 1);
}

#[tokio::test]
async fn retries_embedded_complete_multipart_errors_returned_with_http_200() {
    let server = MockServer::start(|attempt, _| {
        if attempt == 1 {
            Reply::xml(
                StatusCode::OK,
                b"<Error><Code>InternalError</Code><Message>retry later</Message></Error>".to_vec(),
            )
        } else {
            complete_multipart_reply("completed")
        }
    })
    .await;
    let request = CompleteMultipartUploadRequest::new(
        key("completed"),
        UploadId::new("upload-id").expect("valid upload id"),
        vec![CompletedPart::new(1, "\"part-one\"").expect("valid part")],
    )
    .expect("valid completion request");

    let output = client(&server)
        .complete_multipart_upload(request)
        .await
        .expect("retryable embedded error is retried");

    assert_eq!(output.e_tag.as_deref(), Some("\"complete\""));
    assert_eq!(server.request_count().await, 2);
}

#[tokio::test]
async fn copy_metadata_behavior_is_explicit_on_the_wire() {
    let server = MockServer::start(|_, _| {
        Reply::xml(
            StatusCode::OK,
            b"<CopyObjectResult><ETag>\"copied\"</ETag></CopyObjectResult>".to_vec(),
        )
    })
    .await;

    client(&server)
        .copy_object(CopyObjectRequest {
            source: CopySource {
                bucket: "source-bucket".to_owned(),
                key: key("source"),
                version_id: None,
            },
            destination: key("destination"),
            source_conditions: Conditions::default(),
            metadata: CopyMetadataDirective::Replace {
                content_type: Some("text/plain".to_owned()),
                user_metadata: [("purpose".to_owned(), "test".to_owned())]
                    .into_iter()
                    .collect(),
            },
            headers: HeaderMap::new(),
        })
        .await
        .expect("copy succeeds");

    let requests = server.requests().await;
    assert_eq!(
        requests[0].header("x-amz-metadata-directive"),
        Some("REPLACE")
    );
    assert_eq!(requests[0].header("content-type"), Some("text/plain"));
    assert_eq!(requests[0].header("x-amz-meta-purpose"), Some("test"));
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
    assert_eq!(error.attempts(), 1);
    assert_eq!(
        error.retry_stop_reason(),
        Some(RetryStopReason::NonReplayable)
    );
    assert_eq!(server.request_count().await, 1);
    assert_eq!(server.requests().await[0].body, bytes);
}

#[tokio::test]
async fn upload_part_copy_sends_range_and_source_on_the_wire() {
    let server = MockServer::start(|_, _| {
        Reply::xml(
            StatusCode::OK,
            b"<CopyPartResult><LastModified>2024-03-12T10:15:30Z</LastModified><ETag>&quot;part-copy&quot;</ETag></CopyPartResult>".to_vec(),
        )
    })
    .await;
    let mut request = UploadPartCopyRequest::new(
        key("destination"),
        UploadId::new("upload/opaque").unwrap(),
        PartNumber::new(3).unwrap(),
        CopySource {
            bucket: "source-bucket".to_owned(),
            key: key("source key"),
            version_id: Some("version+id".to_owned()),
        },
    );
    request.source_range = Some(CopyPartRange::new(10, 29).unwrap());

    let output = client(&server)
        .upload_part_copy(request)
        .await
        .expect("part copy succeeds");

    assert_eq!(output.e_tag, "\"part-copy\"");
    let requests = server.requests().await;
    assert_eq!(
        requests[0].target,
        "/test-bucket/destination?partNumber=3&uploadId=upload%2Fopaque"
    );
    assert_eq!(
        requests[0].header("x-amz-copy-source"),
        Some("/source-bucket/source%20key?versionId=version%2Bid")
    );
    assert_eq!(
        requests[0].header("x-amz-copy-source-range"),
        Some("bytes=10-29")
    );
}

#[tokio::test]
async fn list_parts_all_advances_markers_on_the_wire() {
    let server = MockServer::start(|attempt, _| {
        if attempt == 1 {
            Reply::xml(
                StatusCode::OK,
                b"<ListPartsResult><IsTruncated>true</IsTruncated><NextPartNumberMarker>1</NextPartNumberMarker><Part><PartNumber>1</PartNumber><ETag>&quot;one&quot;</ETag><Size>5</Size></Part></ListPartsResult>".to_vec(),
            )
        } else {
            Reply::xml(
                StatusCode::OK,
                b"<ListPartsResult><IsTruncated>false</IsTruncated><Part><PartNumber>2</PartNumber><ETag>&quot;two&quot;</ETag><Size>4</Size></Part></ListPartsResult>".to_vec(),
            )
        }
    })
    .await;

    let pages = client(&server)
        .list_parts_all(
            ListPartsRequest::new(key("parts"), UploadId::new("upload-id").unwrap()),
            3,
        )
        .await
        .expect("pagination succeeds");

    assert_eq!(pages.len(), 2);
    let requests = server.requests().await;
    assert!(requests[0].target.contains("uploadId=upload-id"));
    assert!(requests[1].target.contains("part-number-marker=1"));
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
async fn atomic_download_replaces_destination_only_after_verification() {
    let expected = b"verified replacement".to_vec();
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
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("object");
    std::fs::write(&destination, b"old").unwrap();

    let output = client(&server)
        .download_to_path(GetObjectRequest::new(key("atomic")), &destination)
        .await
        .expect("verified download persists");

    assert_eq!(output.bytes_written, expected.len() as u64);
    assert_eq!(std::fs::read(destination).unwrap(), expected);
}

#[tokio::test]
async fn failed_atomic_download_preserves_existing_destination() {
    let server = MockServer::start(|_, _| Reply::Full {
        status: StatusCode::OK,
        headers: vec![
            (
                "x-amz-checksum-sha256".to_owned(),
                sha256_base64(b"different bytes"),
            ),
            ("x-amz-checksum-type".to_owned(), "FULL_OBJECT".to_owned()),
        ],
        body: b"corrupted".to_vec(),
    })
    .await;
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("object");
    std::fs::write(&destination, b"old").unwrap();

    let error = client(&server)
        .download_to_path(GetObjectRequest::new(key("atomic-failure")), &destination)
        .await
        .expect_err("checksum mismatch is not persisted");

    assert_eq!(error.category(), ErrorCategory::Integrity);
    assert_eq!(std::fs::read(destination).unwrap(), b"old");
}

#[tokio::test]
async fn rejects_repeated_pagination_tokens() {
    let body = b"<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>repeat</NextContinuationToken></ListBucketResult>".to_vec();
    let server = MockServer::start(move |_, _| Reply::xml(StatusCode::OK, body.clone())).await;

    let error = client(&server)
        .list_objects_v2_all(ListObjectsV2Request::default(), 5)
        .await
        .expect_err("repeated token must fail");

    assert_eq!(error.category(), ErrorCategory::InvalidResponse);
    assert_eq!(server.request_count().await, 2);
    assert!(
        server
            .requests()
            .await
            .iter()
            .all(|request| request.target.contains("encoding-type=url"))
    );
}

#[tokio::test]
async fn does_not_forward_credentials_or_custom_headers_to_a_custom_redirect_target() {
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
    let mut request = DeleteObjectRequest::new(key("redirected"));
    request.headers.append("x-example", "one".parse().unwrap());
    request.headers.append("x-example", "two".parse().unwrap());

    client(&redirector)
        .delete_object(request)
        .await
        .expect_err("custom redirect is not followed");

    let requests = redirector.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header_values("x-example"), ["one", "two"]);
    assert!(
        requests[0]
            .header("authorization")
            .is_some_and(|value| value.contains("x-example"))
    );
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
        .multipart_upload(managed_bytes(key("managed"), source))
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
async fn managed_multipart_calculates_selected_part_checksum_headers() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("checksum"),
        "PUT" => upload_part_reply("1"),
        "POST" => complete_multipart_reply("checksum"),
        _ => Reply::empty(StatusCode::NOT_FOUND),
    })
    .await;
    let request = managed_bytes(key("checksum"), vec![b'x'; MINIMUM_PART_SIZE])
        .with_checksum_algorithm(ChecksumAlgorithm::Crc32c)
        .expect("CRC32C is supported");

    client(&server)
        .multipart_upload(request)
        .await
        .expect("managed upload succeeds");

    let requests = server.requests().await;
    assert_eq!(
        requests[0].header("x-amz-checksum-algorithm"),
        Some("CRC32C")
    );
    let part = requests
        .iter()
        .find(|request| request.method == "PUT")
        .expect("part request");
    assert!(part.header("x-amz-checksum-crc32c").is_some());
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
        .multipart_upload(managed_bytes(key("part-fails"), source))
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
        .multipart_upload(managed_bytes(key("abort-fails"), source))
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
            .multipart_upload(managed_bytes(key("cancelled"), source))
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

#[tokio::test]
async fn managed_multipart_transfer_deadline_is_end_to_end_and_preserves_cleanup_time() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("deadline"),
        "PUT" => Reply::Stall,
        "DELETE" => Reply::empty(StatusCode::NO_CONTENT),
        _ => Reply::empty(StatusCode::INTERNAL_SERVER_ERROR),
    })
    .await;
    let options = MultipartOptions::new(MINIMUM_PART_SIZE as u64, 1)
        .unwrap()
        .with_transfer_timeout(Duration::from_millis(50))
        .unwrap()
        .with_cleanup_timeout(Duration::from_secs(1))
        .unwrap();
    let request =
        managed_bytes(key("deadline"), vec![b'x'; MINIMUM_PART_SIZE]).with_options(options);

    let error = client(&server)
        .multipart_upload(request)
        .await
        .expect_err("managed transfer deadline expires");

    assert_eq!(error.category(), ErrorCategory::Timeout);
    assert_eq!(error.timeout_phase(), Some(TimeoutPhase::Operation));
    assert!(
        server
            .requests()
            .await
            .iter()
            .any(|request| request.method == "DELETE")
    );
}

#[tokio::test]
async fn upload_part_success_bodies_are_drained_under_a_small_bound() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("body-bound"),
        "PUT" => Reply::Full {
            status: StatusCode::OK,
            headers: vec![("etag".to_owned(), "\"part\"".to_owned())],
            body: vec![b'x'; 8 * 1024 + 1],
        },
        "DELETE" => Reply::empty(StatusCode::NO_CONTENT),
        _ => Reply::empty(StatusCode::INTERNAL_SERVER_ERROR),
    })
    .await;

    let error = client(&server)
        .multipart_upload(managed_bytes(
            key("body-bound"),
            vec![b'x'; MINIMUM_PART_SIZE],
        ))
        .await
        .expect_err("oversized successful part response is rejected");

    assert_eq!(error.category(), ErrorCategory::OversizedResponse);
    assert!(
        server
            .requests()
            .await
            .iter()
            .any(|request| request.method == "DELETE")
    );
}

#[tokio::test]
async fn upload_part_success_body_is_drained_before_headers_are_validated() {
    let server = MockServer::start(|_, request| match request.method.as_str() {
        "POST" if request.target.ends_with("?uploads=") => create_multipart_reply("drain-first"),
        "PUT" => Reply::Full {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: vec![b'x'; 8 * 1024 + 1],
        },
        "DELETE" => Reply::empty(StatusCode::NO_CONTENT),
        _ => Reply::empty(StatusCode::INTERNAL_SERVER_ERROR),
    })
    .await;

    let error = client(&server)
        .multipart_upload(managed_bytes(
            key("drain-first"),
            vec![b'x'; MINIMUM_PART_SIZE],
        ))
        .await
        .expect_err("response body bound is enforced before malformed headers are reported");

    assert_eq!(error.category(), ErrorCategory::OversizedResponse);
    assert!(
        server
            .requests()
            .await
            .iter()
            .any(|request| request.method == "DELETE")
    );
}
