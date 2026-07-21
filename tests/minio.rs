//! Opt-in compatibility tests against a real MinIO server.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use http::header::HOST;
use http::{Method, Request, StatusCode, Uri};
use http_body_util::{BodyExt as _, Full};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use s3_wire::{
    AbortMultipartUploadRequest, ByteRange, ByteStream, CompleteMultipartUploadRequest,
    CompletedPart, CreateMultipartUploadRequest, Credentials, DeleteObjectRequest, Endpoint,
    ErrorCategory, GetObjectRequest, HeadObjectRequest, ListMultipartUploadsRequest,
    ListObjectsV2Request, ManagedMultipartUploadRequest, ObjectKey, PageSize, PartNumber,
    PutObjectRequest, S3Client, S3Config, StaticCredentialsProvider, UploadPartRequest,
};
use sha2::{Digest as _, Sha256};
use tokio::net::TcpStream;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const PART_SIZE: usize = 5 * 1024 * 1024;

fn object_key(value: impl Into<String>) -> ObjectKey {
    ObjectKey::new(value).expect("integration test keys are valid")
}

fn namespace(test: &str) -> String {
    format!("minio/{}/{test}", std::process::id())
}

fn client() -> TestResult<S3Client> {
    let endpoint = std::env::var("MINIO_S3_ENDPOINT")?;
    let bucket = std::env::var("MINIO_S3_BUCKET")?;
    let access_key = std::env::var("MINIO_ROOT_USER")?;
    let secret_key = std::env::var("MINIO_ROOT_PASSWORD")?;
    let credentials = Credentials::new(access_key, secret_key, None)?;
    let config = S3Config::builder()
        .endpoint(Endpoint::new(endpoint)?)
        .allow_http_for_local_testing()
        .bucket(bucket)
        .region("us-east-1")
        .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
        .multipart_threshold(PART_SIZE as u64)
        .multipart_part_size(PART_SIZE as u64)
        .multipart_concurrency(2)
        .max_multipart_in_flight_bytes((2 * PART_SIZE) as u64)
        .build()?;
    Ok(S3Client::new(config)?)
}

async fn collect(mut body: s3_wire::ResponseStream) -> TestResult<Vec<u8>> {
    let mut output = Vec::new();
    while let Some(chunk) = body.next().await {
        output.extend_from_slice(&chunk?);
    }
    Ok(output)
}

async fn delete_if_present(client: &S3Client, key: ObjectKey) {
    let _ = client.delete_object(DeleteObjectRequest::new(key)).await;
}

#[tokio::test]
#[ignore = "run through scripts/test-minio.sh"]
async fn crud_conditions_ranges_metadata_and_unusual_keys() -> TestResult {
    let client = client()?;
    let key = object_key(format!(
        "{}/snow ☃ +?#%/a/b/unusual-payload",
        namespace("crud")
    ));
    let payload = Bytes::from_static(b"0123456789-conditional-range-body");
    let mut metadata = BTreeMap::new();
    metadata.insert("purpose".to_owned(), "minio-compatibility".to_owned());
    metadata.insert("mixed-case".to_owned(), "retained-value".to_owned());

    let mut put = PutObjectRequest::new(key.clone(), ByteStream::from_bytes(payload.clone()));
    put.content_type = Some("application/x-s3-wire-test".to_owned());
    put.user_metadata = metadata.clone();
    let uploaded = client.put_object(put).await?;
    let e_tag = uploaded.e_tag.expect("MinIO returns an ETag");

    let head = client
        .head_object(HeadObjectRequest::new(key.clone()))
        .await?;
    assert_eq!(head.content_length, payload.len() as u64);
    assert_eq!(
        head.content_type.as_deref(),
        Some("application/x-s3-wire-test")
    );
    assert_eq!(head.user_metadata, metadata);
    assert_eq!(head.e_tag.as_deref(), Some(e_tag.as_str()));

    let downloaded = client
        .get_object(GetObjectRequest::new(key.clone()))
        .await?;
    assert_eq!(collect(downloaded.body).await?, payload);

    let mut ranged = GetObjectRequest::new(key.clone());
    ranged.range = Some(ByteRange::inclusive(3, 9)?);
    let ranged = client.get_object(ranged).await?;
    assert_eq!(
        ranged.content_range,
        Some((3, 9, Some(payload.len() as u64)))
    );
    assert_eq!(collect(ranged.body).await?, &payload[3..=9]);

    let mut matching = GetObjectRequest::new(key.clone());
    matching.conditions.if_match = Some(e_tag.clone());
    let matching = client.get_object(matching).await?;
    assert_eq!(collect(matching.body).await?, payload);

    let mut rejected = GetObjectRequest::new(key.clone());
    rejected.conditions.if_match = Some("\"does-not-match\"".to_owned());
    let error = client
        .get_object(rejected)
        .await
        .expect_err("a false ETag precondition must fail");
    assert_eq!(error.category(), ErrorCategory::Precondition);

    let mut create_only = PutObjectRequest::new(
        key.clone(),
        ByteStream::from_bytes(Bytes::from_static(b"must not replace")),
    );
    create_only.conditions.if_none_match = Some("*".to_owned());
    let error = client
        .put_object(create_only)
        .await
        .expect_err("If-None-Match must protect the existing object");
    assert_eq!(error.category(), ErrorCategory::Precondition);

    client
        .delete_object(DeleteObjectRequest::new(key.clone()))
        .await?;
    let error = client
        .head_object(HeadObjectRequest::new(key))
        .await
        .expect_err("deleted object must not exist");
    assert_eq!(error.category(), ErrorCategory::NotFound);
    Ok(())
}

#[tokio::test]
#[ignore = "run through scripts/test-minio.sh"]
async fn pagination_empty_objects_and_concurrent_operations() -> TestResult {
    let client = client()?;
    let prefix = format!("{}/", namespace("pagination"));
    let keys: Vec<ObjectKey> = (0..13)
        .map(|index| object_key(format!("{prefix}{index:02}")))
        .collect();

    let uploads = keys.iter().cloned().map(|key| {
        let client = client.clone();
        async move {
            client
                .put_object(PutObjectRequest::new(
                    key,
                    ByteStream::from_bytes(Bytes::new()),
                ))
                .await
        }
    });
    for result in futures_util::future::join_all(uploads).await {
        result?;
    }

    let mut request = ListObjectsV2Request {
        prefix: Some(prefix.clone()),
        max_keys: PageSize::new(3).expect("three is a valid page size"),
        ..ListObjectsV2Request::default()
    };
    let first_page = client.list_objects_v2(request.clone()).await?;
    assert_eq!(first_page.objects.len(), 3);
    assert!(first_page.is_truncated);
    assert!(first_page.next_continuation_token.is_some());

    request.max_keys = PageSize::new(3).expect("three is a valid page size");
    let pages = client.list_objects_v2_all(request, 10).await?;
    assert!(pages.len() >= 5);
    let listed: BTreeSet<String> = pages
        .into_iter()
        .flat_map(|page| page.objects)
        .map(|object| object.key.into_string())
        .collect();
    let expected: BTreeSet<String> = keys.iter().map(ToString::to_string).collect();
    assert_eq!(listed, expected);

    let downloads = keys.iter().cloned().map(|key| {
        let client = client.clone();
        async move {
            let output = client.get_object(GetObjectRequest::new(key)).await?;
            collect(output.body).await
        }
    });
    for result in futures_util::future::join_all(downloads).await {
        assert!(result?.is_empty());
    }

    let deletes = keys.into_iter().map(|key| {
        let client = client.clone();
        async move { client.delete_object(DeleteObjectRequest::new(key)).await }
    });
    for result in futures_util::future::join_all(deletes).await {
        result?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "run through scripts/test-minio.sh"]
async fn multipart_complete_abort_cleanup_and_managed_upload() -> TestResult {
    let client = client()?;
    let key = object_key(format!("{}/manual", namespace("multipart")));
    let first = Bytes::from(vec![b'a'; PART_SIZE]);
    let second = Bytes::from(vec![b'z'; 1_337]);

    let created = client
        .create_multipart_upload(CreateMultipartUploadRequest::new(key.clone()))
        .await?;
    let upload_id = created.upload_id().clone();
    let part_one = client
        .upload_part(UploadPartRequest::new(
            key.clone(),
            upload_id.clone(),
            PartNumber::new(1).expect("part one is valid"),
            ByteStream::from_bytes(first.clone()),
        ))
        .await?;
    let part_two = client
        .upload_part(UploadPartRequest::new(
            key.clone(),
            upload_id.clone(),
            PartNumber::new(2).expect("part two is valid"),
            ByteStream::from_bytes(second.clone()),
        ))
        .await?;
    let completed_parts = vec![
        CompletedPart::new(1, part_one.e_tag)?,
        CompletedPart::new(2, part_two.e_tag)?,
    ];
    client
        .complete_multipart_upload(CompleteMultipartUploadRequest::new(
            key.clone(),
            upload_id,
            completed_parts,
        )?)
        .await?;
    let object = client
        .get_object(GetObjectRequest::new(key.clone()))
        .await?;
    let bytes = collect(object.body).await?;
    assert_eq!(bytes.len(), first.len() + second.len());
    assert_eq!(&bytes[..first.len()], first.as_ref());
    assert_eq!(&bytes[first.len()..], second.as_ref());
    delete_if_present(&client, key).await;

    let abort_key = object_key(format!("{}/abort", namespace("multipart")));
    let created = client
        .create_multipart_upload(CreateMultipartUploadRequest::new(abort_key.clone()))
        .await?;
    let abort_id = created.upload_id().clone();
    let mut listings = ListMultipartUploadsRequest {
        prefix: Some(abort_key.to_string()),
        ..ListMultipartUploadsRequest::default()
    };
    let active = client.list_multipart_uploads(listings.clone()).await?;
    assert!(
        active
            .uploads
            .iter()
            .any(|upload| upload.key == abort_key && upload.upload_id() == &abort_id)
    );
    client
        .abort_multipart_upload(AbortMultipartUploadRequest::new(
            abort_key.clone(),
            abort_id,
        ))
        .await?;
    listings.prefix = Some(abort_key.to_string());
    let active = client.list_multipart_uploads(listings).await?;
    assert!(active.uploads.iter().all(|upload| upload.key != abort_key));

    let managed_key = object_key(format!("{}/managed", namespace("multipart")));
    let managed_body = Bytes::from(vec![0x5a; PART_SIZE + 2_111]);
    client
        .multipart_upload(ManagedMultipartUploadRequest::from_bytes(
            managed_key.clone(),
            managed_body.clone(),
        ))
        .await?;
    let object = client
        .get_object(GetObjectRequest::new(managed_key.clone()))
        .await?;
    assert_eq!(collect(object.body).await?, managed_body);
    delete_if_present(&client, managed_key).await;
    Ok(())
}

#[tokio::test]
#[ignore = "run through scripts/test-minio.sh"]
async fn large_one_shot_stream_and_presigned_urls() -> TestResult {
    let client = client()?;
    let streamed_key = object_key(format!("{}/streamed", namespace("presign")));
    let chunks: Vec<Bytes> = (0..9)
        .map(|index| Bytes::from(vec![index; 1024 * 1024]))
        .collect();
    let length = chunks.iter().map(Bytes::len).sum::<usize>();
    let mut hasher = Sha256::new();
    for chunk in &chunks {
        hasher.update(chunk);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let upload_stream = stream::iter(chunks.clone().into_iter().map(Ok::<Bytes, io::Error>));
    client
        .put_object(PutObjectRequest::new(
            streamed_key.clone(),
            ByteStream::from_stream(upload_stream, length as u64, digest),
        ))
        .await?;
    let object = client
        .get_object(GetObjectRequest::new(streamed_key.clone()))
        .await?;
    let downloaded = collect(object.body).await?;
    assert_eq!(downloaded.len(), length);
    let downloaded_digest: [u8; 32] = Sha256::digest(downloaded).into();
    assert_eq!(downloaded_digest, digest);
    delete_if_present(&client, streamed_key).await;

    let presigned_key = object_key(format!("{}/signed URL +☃", namespace("presign")));
    let payload = Bytes::from_static(b"presigned request body");
    let put_url = client
        .presigned_put(&presigned_key, Duration::from_secs(60))
        .await?;
    let (status, _) = raw_http(Method::PUT, put_url.expose(), payload.clone()).await?;
    assert_eq!(status, StatusCode::OK);

    let get_url = client
        .presigned_get(&presigned_key, Duration::from_secs(60))
        .await?;
    let (status, downloaded) = raw_http(Method::GET, get_url.expose(), Bytes::new()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded, payload);
    delete_if_present(&client, presigned_key).await;
    Ok(())
}

async fn raw_http(method: Method, url: &str, body: Bytes) -> TestResult<(StatusCode, Bytes)> {
    let uri: Uri = url.parse()?;
    let authority = uri
        .authority()
        .ok_or("presigned URL has no authority")?
        .clone();
    let request_target = uri
        .path_and_query()
        .ok_or("presigned URL has no request target")?
        .as_str();
    let socket = TcpStream::connect(authority.as_str()).await?;
    let (mut sender, connection) = http1::handshake(TokioIo::new(socket)).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = Request::builder()
        .method(method)
        .uri(request_target)
        .header(HOST, authority.as_str())
        .body(Full::new(body))?;
    let response = sender.send_request(request).await?;
    let status = response.status();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((status, bytes))
}
