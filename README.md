# s3-wire

[![crates.io](https://img.shields.io/crates/v/s3-wire.svg)](https://crates.io/crates/s3-wire)
[![docs.rs](https://docs.rs/s3-wire/badge.svg)](https://docs.rs/s3-wire)
[![CI](https://github.com/runtrue/s3-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/runtrue/s3-rs/actions/workflows/ci.yml)

Async, streaming S3-compatible client for Rust with explicit bounds on memory, retries, timeouts, and remote input.

## Highlights

- Tokio-native uploads and downloads with backpressure
- Replay-aware retries for in-memory and file-backed request bodies
- Managed multipart uploads with bounded concurrency and abort cleanup
- SigV4 request signing and presigned GET and PUT URLs
- Typed object keys, ranges, conditions, checksums, and multipart state
- HTTPS by default, secret-redacting types, and bounded XML parsing
- Integration-tested against pinned MinIO, RustFS, and SeaweedFS releases

`s3-wire` requires Rust 1.97.1 and does not depend on another S3 client.

## Install

```sh
cargo add s3-wire
cargo add tokio --features fs,macros,rt-multi-thread
```

The default transport uses HTTP/1.1. Enable the `http2` crate feature when an
endpoint and workload benefit from HTTP/2 negotiation.

## Quick start

The default credential provider reads `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optional `AWS_SESSION_TOKEN`. Remote endpoints must use HTTPS; plain HTTP is an explicit local-testing opt-in.

```rust,no_run
use s3_wire::{
    ByteStream, Endpoint, GetObjectRequest, ObjectKey, PutObjectRequest, S3Client, S3Config,
};

async fn put_then_get() -> Result<(), Box<dyn std::error::Error>> {
    let region = "us-east-1";
    let config = S3Config::builder()
        .endpoint(Endpoint::for_aws_region(region)?)
        .region(region)
        .bucket("artifact-bucket")
        .build()?;
    let client = S3Client::new(config)?;

    let key = ObjectKey::new("artifacts/report.json")?;
    let mut upload = PutObjectRequest::new(
        key.clone(),
        ByteStream::from_bytes(br#"{"status":"complete"}"#.as_slice()),
    );
    upload.content_type = Some("application/json".into());
    client.put_object(upload).await?;

    let download = client.get_object(GetObjectRequest::new(key)).await?;
    let mut destination = tokio::fs::File::create("report.json").await?;
    let written = download.body.write_to(&mut destination).await?;

    println!("downloaded {written} bytes");
    Ok(())
}
```

Run the complete CRUD, range, listing, and conditional-write example with:

```sh
cargo run --example basic
```

The example uses the standard AWS credential variables plus `S3_BUCKET`, and supports both AWS and custom endpoints.

## Configuration and credentials

`S3Config` validates addressing, timeouts, retry policy, response limits, and the credential provider before a client is created:

```rust,no_run
use std::sync::Arc;
use std::time::Duration;

use s3_wire::{
    AddressingStyle, CachedCredentialsProvider, Credentials, Endpoint, RetryPolicy, S3Client,
    S3Config, StaticCredentialsProvider,
};

fn configured_client() -> Result<S3Client, Box<dyn std::error::Error>> {
    let credentials = Credentials::new("access-key", "secret-key", None)?;
    let static_provider = Arc::new(StaticCredentialsProvider::new(credentials));
    let cached_provider = Arc::new(CachedCredentialsProvider::new(static_provider));
    let retries = RetryPolicy::new(
        4,
        Duration::from_millis(100),
        Duration::from_secs(5),
        Duration::from_secs(20),
    )?;

    let config = S3Config::builder()
        .endpoint(Endpoint::new("https://s3.example.com")?)
        .region("us-east-1")
        .bucket("artifact-bucket")
        .addressing_style(AddressingStyle::Path)
        .connect_timeout(Duration::from_secs(5))
        .attempt_timeout(Duration::from_secs(30))
        .operation_timeout(Duration::from_secs(5 * 60))
        .idle_body_timeout(Duration::from_secs(30))
        .retry_policy(retries)
        .credentials_provider(cached_provider)
        .build()?;

    Ok(S3Client::new(config)?)
}
```

The default `EnvironmentCredentialsProvider` needs no explicit configuration. Use `StaticCredentialsProvider` for an injected immutable value, or implement the async `CredentialsProvider` trait for a workload-specific source. `CachedCredentialsProvider` coalesces concurrent refreshes and respects credential expiration.

## Upload sources

Choose a body based on how it should behave if a request must be retried:

| Source | Replayable | Memory behavior | Notes |
| --- | --- | --- | --- |
| `ByteStream::from_bytes` | Yes | Retains the input bytes | Best for small, already-buffered values |
| `ByteStream::from_path` | Yes | Streams from a private disk snapshot | Requires temporary disk space |
| `ByteStream::from_stream` | No | Streams with backpressure | Caller supplies exact length and SHA-256 |

File uploads are hashed into an immutable temporary snapshot before the first request. A retry therefore sends the same bytes even if the original file changes.

## Multipart uploads

Managed multipart handles part scheduling, a derived in-flight byte bound, one
end-to-end transfer deadline, completion, and separately bounded cleanup that
quiesces in-flight part requests before aborting:

```rust,no_run
use std::time::Duration;

use s3_wire::{ManagedMultipartUploadRequest, MultipartOptions, ObjectKey, S3Client};

async fn upload_large_file(client: &S3Client) -> Result<(), Box<dyn std::error::Error>> {
    let options = MultipartOptions::new(8 * 1024 * 1024, 4)?
        .with_transfer_timeout(Duration::from_secs(15 * 60))?;
    let request =
        ManagedMultipartUploadRequest::from_path(
            ObjectKey::new("artifacts/archive.tar")?,
            "archive.tar",
        )
        .with_content_type("application/x-tar")
        .with_options(options);

    client.multipart_upload(request).await?;
    Ok(())
}
```

Multipart selection is intentional: `put_object` never switches modes automatically. Call `multipart_upload` when application policy says a source should use multipart. Primitive create, upload-part, complete, list, and abort operations are also available when the application must own multipart state.

Dropping a managed upload stops scheduling parts, gives transmitted requests a
bounded opportunity to settle, and then attempts an abort after an upload ID
exists. If requests cannot settle, the error exposes cleanup failure because
abort cannot be guaranteed to win that race. Process termination can still
leave stale uploads, so long-running deployments should also run bounded
stale-upload cleanup.

## Listing and presigning

`list_objects_v2_all` follows continuation tokens up to a caller-supplied page limit. `presigned_get` and `presigned_put` return a redacted `PresignedUrl`:

```rust,no_run
use std::time::Duration;

use s3_wire::{ObjectKey, S3Client};

async fn share_download(client: &S3Client) -> Result<(), Box<dyn std::error::Error>> {
    let key = ObjectKey::new("artifacts/report.json")?;
    let url = client
        .presigned_get(&key, Duration::from_secs(300))
        .await?;

    // Exposure is explicit because the query string contains signing material.
    send_to_authorized_caller(url.expose());
    Ok(())
}

fn send_to_authorized_caller(_url: &str) {}
```

Presigned URLs are bearer credentials. Keep their lifetime short and do not place exposed URLs in logs, analytics, or error messages.

## Error handling

`S3Error` separates a stable category from optional service metadata. Its `Display` and `Debug` implementations omit transport text and cleanup details that may contain credentials or signed URLs:

```rust
use s3_wire::{ErrorCategory, S3Error};

fn report(error: &S3Error) {
    match error.category() {
        ErrorCategory::NotFound => eprintln!("object does not exist"),
        ErrorCategory::Timeout => eprintln!("timeout during {:?}", error.timeout_phase()),
        ErrorCategory::Throttling => eprintln!("request was throttled"),
        _ => eprintln!("S3 request failed: {}", error.message()),
    }

    if let Some(request_id) = error.request_id() {
        eprintln!("request id: {request_id}");
    }
    if error.cleanup_failure().is_some() {
        eprintln!("multipart cleanup also failed");
    }
}
```

Retries are applied inside the client only when the classification, attempt and elapsed-time limits, operation deadline, and body replayability all permit another attempt.

## Examples

Focused, runnable examples live in [`examples/README.md`](examples/README.md):

- [Basic object lifecycle](examples/basic.rs)
- [Streaming download to a file](examples/download_file.rs)
- [One-shot streaming upload](examples/stream_upload.rs)
- [Managed multipart upload](examples/multipart_upload.rs)
- [Presigned GET and PUT](examples/presign.rs)
- [Copy and batch delete](examples/copy_and_delete.rs)
- [Content-addressed artifact-store adapter](examples/artifact_store.rs)

## Compatibility

The client currently covers object upload, download, inspection, deletion, batch deletion, server-side copy, listing, range reads, conditional headers, presigning, and primitive or managed multipart uploads.

The pinned MinIO, RustFS, and SeaweedFS suites run in CI. An opt-in AWS suite exists but has not yet been executed for this release, so compatible-server results are not presented as proof of AWS compatibility. See [S3 compatibility](docs/compatibility.md) for the operation matrix, checksum behavior, test status, and unsupported API families.

## Scope and limits

- The client is async-only and configured for one bucket at a time.
- AWS chunked SigV4 streaming is not implemented.
- Automatic upload-checksum calculation currently supports SHA-256.
- Managed multipart does not expose a destination `If-None-Match` condition.
- Bucket administration, ACLs, policies, version listing, metadata-service credentials, and encryption configuration are outside the current API.

See the [security model](docs/security-model.md) for deployment responsibilities and [architecture](docs/architecture.md) for retry, transport, and ownership details.

## Documentation

| Guide | What it covers |
| --- | --- |
| [API reference](https://docs.rs/s3-wire) | Public types, methods, and crate-level quick start |
| [Architecture](docs/architecture.md) | Modules, request flow, retry rules, and transfer ownership |
| [S3 compatibility](docs/compatibility.md) | Operations, signing, checksums, tested services, and non-goals |
| [Security model](docs/security-model.md) | Trust boundaries, controls, secret handling, and operator duties |
| [Testing](docs/testing.md) | Local checks, MinIO, AWS, property tests, fuzzing, and benchmarks |
| [Performance and size](docs/performance.md) | Retained measurements, reproduction, and interpretation |
| [sandboxd integration](docs/sandboxd-integration.md) | Content-addressed publication, reads, garbage collection, and cleanup |
| [Examples](examples/README.md) | Runnable object, streaming, multipart, presigning, and integration flows |
| [Releasing](docs/releasing.md) | Package validation, tagging, publication, and post-release checks |

Also see [the API example](examples/basic.rs), [security reporting](SECURITY.md), and [contributing](CONTRIBUTING.md).

## Development

Run the fast validation set after local changes:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
```

The pinned endpoint suites run with `./scripts/test-s3-compat.sh <minio|rustfs|seaweedfs>`. Packaging and release checks are described in [testing](docs/testing.md).

## License

Licensed under the [Apache License 2.0](LICENSE).
