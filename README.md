# s3-wire

`s3-wire` is a Tokio-native S3-compatible client for bounded object transfers. It implements SigV4, typed object keys, replay-aware retries, streaming downloads, and managed or primitive multipart uploads without depending on another S3 client.

The crate requires Rust 1.97.1.

```toml
[dependencies]
s3-wire = "0.1"
tokio = { version = "1", features = ["fs", "macros", "rt-multi-thread"] }
```

## Client configuration

The default credential provider reads `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optional `AWS_SESSION_TOKEN`. HTTPS is required unless local-test HTTP is explicitly enabled.

```rust,no_run
use s3_wire::{AddressingStyle, Endpoint, S3Client, S3Config, S3Error};

fn client() -> Result<S3Client, S3Error> {
    let region = "us-east-1";
    let config = S3Config::builder()
        .endpoint(Endpoint::for_aws_region(region)?)
        .region(region)
        .bucket("artifact-bucket")
        .addressing_style(AddressingStyle::Path)
        .build()?;
    S3Client::new(config)
}
```

Use `StaticCredentialsProvider` or implement the async `CredentialsProvider` trait when credentials do not come from the environment. `CachedCredentialsProvider` serializes refreshes and respects credential expiration.

## Object transfers

Upload in-memory bytes:

```rust,no_run
# use s3_wire::{ByteStream, ObjectKey, PutObjectRequest, S3Error};
# async fn upload(client: &s3_wire::S3Client) -> Result<(), S3Error> {
let key = ObjectKey::new("artifacts/report.json")?;
let mut request = PutObjectRequest::new(
    key,
    ByteStream::from_bytes(br#"{"status":"complete"}"#.as_slice()),
);
request.content_type = Some("application/json".into());
client.put_object(request).await?;
# Ok(())
# }
```

Upload a file without retaining the complete file in memory. The client first hashes the file into an immutable disk-backed snapshot, then streams that snapshot; retries read the same bytes.

```rust,no_run
# use s3_wire::{ByteStream, ObjectKey, PutObjectRequest, S3Error};
# async fn upload_file(client: &s3_wire::S3Client) -> Result<(), S3Error> {
let request = PutObjectRequest::new(
    ObjectKey::new("artifacts/archive.tar")?,
    ByteStream::from_path("archive.tar"),
);
client.put_object(request).await?;
# Ok(())
# }
```

Stream a download to any Tokio `AsyncWrite` implementation:

```rust,no_run
# use s3_wire::{GetObjectRequest, ObjectKey, S3Error};
# async fn download(client: &s3_wire::S3Client) -> Result<(), S3Error> {
let response = client
    .get_object(GetObjectRequest::new(ObjectKey::new("artifacts/archive.tar")?))
    .await?;
let mut destination = tokio::fs::File::create("archive.tar")
    .await
    .map_err(S3Error::transport)?;
response.body.write_to(&mut destination).await?;
# Ok(())
# }
```

`ResponseStream` also implements `futures_core::Stream<Item = Result<bytes::Bytes, S3Error>>` for callers that process chunks directly. Range requests use `GetObjectRequest::range` and `ByteRange`.

For immutable publication, set `If-None-Match: *`:

```rust,no_run
# use s3_wire::{ByteStream, ObjectKey, PutObjectRequest, S3Error};
# async fn create(client: &s3_wire::S3Client) -> Result<(), S3Error> {
let mut request = PutObjectRequest::new(
    ObjectKey::new("manifests/sha256.json")?,
    ByteStream::from_bytes(b"{}".as_slice()),
);
request.conditions.if_none_match = Some("*".into());
client.put_object(request).await?;
# Ok(())
# }
```

## Multipart uploads

Managed multipart upload bounds concurrent part buffers using `multipart_part_size`, `multipart_concurrency`, and `max_multipart_in_flight_bytes` from `S3Config`:

```rust,no_run
# use s3_wire::{ManagedMultipartUploadRequest, ObjectKey, S3Error};
# async fn managed(client: &s3_wire::S3Client) -> Result<(), S3Error> {
let request = ManagedMultipartUploadRequest::from_path(
    ObjectKey::new("artifacts/large-image.raw")?,
    "large-image.raw",
)
.with_content_type("application/octet-stream");
client.multipart_upload(request).await?;
# Ok(())
# }
```

Dropping the managed upload future cancels outstanding parts. If S3 has created an upload, the client-owned task retains the upload ID and attempts abort. A normal upload failure waits for abort; if abort fails, `S3Error::cleanup_failure()` preserves the cleanup result alongside the primary failure. The managed operation does not expose a destination `If-None-Match` precondition, so callers publishing immutable names must serialize writers or use a separate publication protocol.

Primitive multipart operations expose state explicitly:

```rust,no_run
# use s3_wire::{ByteStream, CompleteMultipartUploadRequest, CompletedPart, CreateMultipartUploadRequest, ObjectKey, PartNumber, S3Error, UploadPartRequest};
# async fn primitive(client: &s3_wire::S3Client) -> Result<(), Box<dyn std::error::Error>> {
let key = ObjectKey::new("artifacts/parts.bin")?;
let created = client
    .create_multipart_upload(CreateMultipartUploadRequest::new(key.clone()))
    .await?;
let upload_id = created.upload_id().clone();

let first = client
    .upload_part(UploadPartRequest::new(
        key.clone(),
        upload_id.clone(),
        PartNumber::new(1).expect("one is a valid part number"),
        ByteStream::from_bytes(vec![0_u8; 5 * 1024 * 1024]),
    ))
    .await?;
let completed = CompletedPart::new(1, first.e_tag)?.with_checksum(first.checksum);
client
    .complete_multipart_upload(CompleteMultipartUploadRequest::new(
        key,
        upload_id,
        vec![completed],
    )?)
    .await?;
# Ok(())
# }
```

Primitive callers own abort cleanup and should retain `UploadId` until completion or a successful `abort_multipart_upload`.

## Listing and presigning

```rust,no_run
# use std::time::Duration;
# use s3_wire::{ListObjectsV2Request, ObjectKey, S3Error};
# async fn list_and_presign(client: &s3_wire::S3Client) -> Result<(), S3Error> {
let pages = client
    .list_objects_v2_all(
        ListObjectsV2Request {
            prefix: Some("artifacts/".into()),
            ..ListObjectsV2Request::default()
        },
        100,
    )
    .await?;
for object in pages.iter().flat_map(|page| &page.objects) {
    println!("{}", object.key);
}

let key = ObjectKey::new("artifacts/report.json")?;
let signed = client.presigned_get(&key, Duration::from_secs(300)).await?;
// Exposure is explicit because the query string contains signing material.
send_url_to_authorized_caller(signed.expose());
# Ok(())
# }
# fn send_url_to_authorized_caller(_: &str) {}
```

## Supported operations

The public client includes `PutObject`, `GetObject`, `HeadObject`, `DeleteObject`, `DeleteObjects`, `CopyObject`, `ListObjectsV2`, create/upload/complete/abort multipart, multipart listing, range reads, conditional headers, and presigned GET and PUT URLs. See [compatibility](docs/compatibility.md) for operation details and test status.

## Limits and non-goals

- Arbitrary AWS chunked SigV4 uploads are not implemented. `ByteStream::from_stream` requires the exact length and SHA-256 digest and is one-shot, so it is not retried.
- In-memory and file-backed bodies are replayable. File-backed bodies use a private disk snapshot; callers must provide enough temporary storage.
- Multipart selection is explicit: call `multipart_upload` when the source should use multipart. `multipart_threshold` is validated configuration available to application policy; `put_object` does not switch modes automatically.
- PutObject can calculate a SHA-256 checksum. CRC32, CRC32C, CRC64/NVME, and SHA-1 upload calculation are not implemented. Primitive multipart accepts validated caller-supplied base64 checksums. Full-object SHA-256 download checksums are verified at end of stream; other returned checksum values are exposed but not recalculated.
- Managed multipart has no destination precondition. Cancellation owns abort cleanup, but process termination cannot complete an in-flight network cleanup; stale uploads should also be reclaimed through a bounded listing policy.
- Metadata-service credential providers, bucket administration, ACLs, policies, version listing, and object encryption configuration are outside this crate's current API.
- The pinned MinIO, RustFS, and SeaweedFS suites run in CI. The real AWS compatibility suite is implemented as an opt-in test but has not yet been executed for this release, so compatible-server results are not evidence of AWS compatibility.

## Documentation

- [Architecture](docs/architecture.md)
- [Security model](docs/security-model.md)
- [Compatibility](docs/compatibility.md)
- [Testing](docs/testing.md)
- [Performance and size](docs/performance.md)
- [sandboxd integration contract](docs/sandboxd-integration.md)
- [Security reporting](SECURITY.md)
- [Contributing](CONTRIBUTING.md)

The retained [microbenchmark baseline](measurements/microbench-2026-07-21.md), [local MinIO measurement](measurements/results/minio-local.md), and [release-build comparison](comparisons/size/REPORT.md) record their toolchains and caveats. They are measurements of specific runs, not general performance or compatibility claims.
