//! Opt-in compatibility checks against an existing AWS S3 bucket.

#![cfg(feature = "aws-compat")]

use std::env;
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::{Method, Request};
use http_body_util::{BodyExt as _, Full};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client as HttpClient;
use hyper_util::rt::TokioExecutor;
use s3_wire::{
    AbortMultipartUploadRequest, AddressingStyle, ByteRange, ByteStream,
    CompleteMultipartUploadRequest, CompletedPart, CreateMultipartUploadRequest, Credentials,
    DeleteObjectIdentifier, DeleteObjectRequest, DeleteObjectsRequest, Endpoint, ErrorCategory,
    GetObjectRequest, HeadObjectRequest, ListObjectsV2Request, ObjectKey, PartNumber,
    PutObjectRequest, S3Client, S3Config, StaticCredentialsProvider, UploadId, UploadPartRequest,
};

const FIRST_PART_SIZE: usize = 5 * 1024 * 1024;
type TestError = Box<dyn Error + Send + Sync>;
type TestResult<T = ()> = Result<T, TestError>;

struct CleanupState {
    keys: Vec<ObjectKey>,
    uploads: Vec<(ObjectKey, UploadId)>,
}

impl CleanupState {
    fn new() -> Self {
        Self {
            keys: Vec::new(),
            uploads: Vec::new(),
        }
    }
}

#[tokio::test]
#[ignore = "requires AWS credentials and S3_WIRE_AWS_BUCKET"]
async fn aws_crud_range_conditions_listing_multipart_and_presign() -> TestResult {
    let client = client_from_environment()?;
    let prefix = unique_prefix()?;
    let mut cleanup = CleanupState::new();

    let result = exercise_aws(&client, &prefix, &mut cleanup).await;
    let cleanup_result = cleanup_aws(&client, cleanup).await;
    match (result, cleanup_result) {
        (Err(primary), _) => Err(primary),
        (Ok(()), cleanup) => cleanup,
    }
}

async fn exercise_aws(client: &S3Client, prefix: &str, cleanup: &mut CleanupState) -> TestResult {
    let object = key(prefix, "crud.txt")?;
    cleanup.keys.push(object.clone());
    let payload = Bytes::from_static(b"hello, compatibility");
    let mut put = PutObjectRequest::new(object.clone(), ByteStream::from_bytes(payload.clone()));
    put.content_type = Some("text/plain".to_owned());
    put.conditions.if_none_match = Some("*".to_owned());
    client.put_object(put).await?;

    let head = client
        .head_object(HeadObjectRequest::new(object.clone()))
        .await?;
    require(
        head.content_length == u64::try_from(payload.len())?,
        "HEAD returned an unexpected content length",
    )?;

    let duplicate = PutObjectRequest::new(
        object.clone(),
        ByteStream::from_bytes(Bytes::from_static(b"must not replace")),
    );
    let mut duplicate = duplicate;
    duplicate.conditions.if_none_match = Some("*".to_owned());
    let error = match client.put_object(duplicate).await {
        Ok(_) => {
            return Err(io::Error::other("create-only replacement unexpectedly succeeded").into());
        }
        Err(error) => error,
    };
    require(
        error.category() == ErrorCategory::Precondition,
        "create-only replacement did not return a precondition failure",
    )?;

    let download = client
        .get_object(GetObjectRequest::new(object.clone()))
        .await?;
    let mut downloaded = Vec::new();
    download.body.write_to(&mut downloaded).await?;
    require(downloaded == payload, "full GET returned different bytes")?;

    let mut range = GetObjectRequest::new(object.clone());
    range.range = Some(ByteRange::inclusive(7, 19)?);
    let download = client.get_object(range).await?;
    let mut ranged = Vec::new();
    download.body.write_to(&mut ranged).await?;
    require(
        ranged == b"compatibility",
        "range GET returned different bytes",
    )?;

    let pages = client
        .list_objects_v2_all(
            ListObjectsV2Request {
                prefix: Some(prefix.to_owned()),
                ..ListObjectsV2Request::default()
            },
            10,
        )
        .await?;
    require(
        pages
            .iter()
            .flat_map(|page| &page.objects)
            .any(|listed| listed.key == object),
        "listing did not include the uploaded object",
    )?;

    exercise_multipart(client, prefix, cleanup).await?;
    exercise_presigning(client, prefix, cleanup).await?;

    client
        .delete_object(DeleteObjectRequest::new(object.clone()))
        .await?;
    let missing = match client.head_object(HeadObjectRequest::new(object)).await {
        Ok(_) => return Err(io::Error::other("deleted object still exists").into()),
        Err(error) => error,
    };
    require(
        missing.category() == ErrorCategory::NotFound,
        "HEAD after DELETE did not return not-found",
    )?;
    Ok(())
}

async fn exercise_multipart(
    client: &S3Client,
    prefix: &str,
    cleanup: &mut CleanupState,
) -> TestResult {
    let object = key(prefix, "multipart.bin")?;
    cleanup.keys.push(object.clone());
    let created = client
        .create_multipart_upload(CreateMultipartUploadRequest::new(object.clone()))
        .await?;
    let upload_id = created.upload_id().clone();
    cleanup.uploads.push((object.clone(), upload_id.clone()));

    let first = client
        .upload_part(UploadPartRequest::new(
            object.clone(),
            upload_id.clone(),
            PartNumber::new(1).ok_or_else(|| io::Error::other("invalid part number"))?,
            ByteStream::from_bytes(vec![b'a'; FIRST_PART_SIZE]),
        ))
        .await?;
    let second = client
        .upload_part(UploadPartRequest::new(
            object.clone(),
            upload_id.clone(),
            PartNumber::new(2).ok_or_else(|| io::Error::other("invalid part number"))?,
            ByteStream::from_bytes(Bytes::from_static(b"tail")),
        ))
        .await?;
    let completed = vec![
        CompletedPart::new(1, first.e_tag)?.with_checksum(first.checksum),
        CompletedPart::new(2, second.e_tag)?.with_checksum(second.checksum),
    ];
    client
        .complete_multipart_upload(CompleteMultipartUploadRequest::new(
            object.clone(),
            upload_id.clone(),
            completed,
        )?)
        .await?;
    cleanup
        .uploads
        .retain(|(key, id)| key != &object || id != &upload_id);

    let head = client.head_object(HeadObjectRequest::new(object)).await?;
    require(
        head.content_length == u64::try_from(FIRST_PART_SIZE + 4)?,
        "completed multipart object has an unexpected length",
    )?;
    Ok(())
}

async fn exercise_presigning(
    client: &S3Client,
    prefix: &str,
    cleanup: &mut CleanupState,
) -> TestResult {
    let object = key(prefix, "presigned.txt")?;
    cleanup.keys.push(object.clone());
    let payload = Bytes::from_static(b"presigned request body");

    let put_url = client
        .presigned_put(&object, Duration::from_secs(300))
        .await?;
    let response = unsigned_http(Method::PUT, put_url.expose(), payload.clone()).await?;
    require(
        response.is_empty(),
        "presigned PUT returned an unexpected body",
    )?;

    let get_url = client
        .presigned_get(&object, Duration::from_secs(300))
        .await?;
    let response = unsigned_http(Method::GET, get_url.expose(), Bytes::new()).await?;
    require(
        response == payload,
        "presigned GET returned different bytes",
    )?;
    Ok(())
}

async fn unsigned_http(method: Method, url: &str, body: Bytes) -> TestResult<Bytes> {
    let connector = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let client: HttpClient<_, Full<Bytes>> =
        HttpClient::builder(TokioExecutor::new()).build(connector);
    let request = Request::builder()
        .method(method)
        .uri(url)
        .body(Full::new(body))?;
    let response = client.request(request).await?;
    require(
        response.status().is_success(),
        "presigned HTTP request returned a non-success status",
    )?;
    Ok(response.into_body().collect().await?.to_bytes())
}

async fn cleanup_aws(client: &S3Client, cleanup: CleanupState) -> TestResult {
    let mut first_error: Option<TestError> = None;
    for (key, upload_id) in cleanup.uploads {
        if let Err(error) = client
            .abort_multipart_upload(AbortMultipartUploadRequest::new(key, upload_id))
            .await
        {
            first_error.get_or_insert_with(|| Box::new(error));
        }
    }
    for batch in cleanup.keys.chunks(1_000) {
        let request = DeleteObjectsRequest::new(
            batch
                .iter()
                .cloned()
                .map(DeleteObjectIdentifier::new)
                .collect(),
        )?;
        match client.delete_objects(request).await {
            Ok(output) if output.errors.is_empty() => {}
            Ok(_) => {
                first_error.get_or_insert_with(|| {
                    Box::new(io::Error::other(
                        "AWS cleanup returned per-object deletion errors",
                    ))
                });
            }
            Err(error) => {
                first_error.get_or_insert_with(|| Box::new(error));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn client_from_environment() -> TestResult<S3Client> {
    let bucket = required_env("S3_WIRE_AWS_BUCKET")?;
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
    let endpoint = env::var("AWS_ENDPOINT_URL")
        .map_or_else(|_| Endpoint::for_aws_region(&region), Endpoint::new)?;
    let credentials = Credentials::new(
        required_env("AWS_ACCESS_KEY_ID")?,
        required_env("AWS_SECRET_ACCESS_KEY")?,
        env::var("AWS_SESSION_TOKEN").ok(),
    )?;
    let style = match env::var("S3_WIRE_AWS_ADDRESSING_STYLE").as_deref() {
        Ok("virtual") => AddressingStyle::VirtualHosted,
        _ => AddressingStyle::Path,
    };
    let mut builder = S3Config::builder()
        .endpoint(endpoint.clone())
        .region(region)
        .bucket(bucket.clone())
        .addressing_style(style)
        .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
        .connect_timeout(Duration::from_secs(10))
        .attempt_timeout(Duration::from_secs(60))
        .operation_timeout(Duration::from_secs(5 * 60));
    if !endpoint.is_https() {
        builder = builder.allow_http_for_local_testing();
    }
    Ok(S3Client::new(builder.build()?)?)
}

fn unique_prefix() -> TestResult<String> {
    let elapsed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
    Ok(format!(
        "s3-wire-compat/{}-{}-{:016x}/",
        elapsed.as_nanos(),
        std::process::id(),
        fastrand::u64(..)
    ))
}

fn key(prefix: &str, suffix: &str) -> TestResult<ObjectKey> {
    Ok(ObjectKey::new(format!("{prefix}{suffix}"))?)
}

fn required_env(name: &'static str) -> TestResult<String> {
    env::var(name)
        .map_err(|_| io::Error::other(format!("required environment variable {name} is not set")))
        .map_err(Into::into)
}

fn require(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}
