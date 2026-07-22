# Examples

Every example is runnable and compile-checked as part of the test suite.

## Environment

The focused examples use the standard AWS credential variables:

- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`
- `AWS_SESSION_TOKEN` (optional)

They also accept:

- `S3_BUCKET`: required target bucket
- `S3_REGION`: signing region; defaults to `us-east-1`
- `S3_ENDPOINT`: optional custom endpoint

Plain HTTP is enabled only when `S3_ENDPOINT` explicitly selects an HTTP URL. Use it only for a loopback or trusted local test service.

The application-oriented `artifact_store` example instead accepts `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, and optional `S3_SESSION_TOKEN` to demonstrate injected static credentials at an adapter boundary.

## Object operations

- [`basic.rs`](basic.rs): conditional PUT, HEAD, range GET, listing, and DELETE
- [`download_file.rs`](download_file.rs): stream an object directly to a Tokio file
- [`stream_upload.rs`](stream_upload.rs): one-shot upload with an exact length and SHA-256 digest
- [`copy_and_delete.rs`](copy_and_delete.rs): server-side copy and bounded batch delete

## Multipart and presigning

- [`multipart_upload.rs`](multipart_upload.rs): bounded managed multipart upload from a file; requires `S3_FILE`
- [`presign.rs`](presign.rs): short-lived GET and PUT URLs with explicit secret exposure

## Integration pattern

- [`artifact_store.rs`](artifact_store.rs): content-addressed, tenant-isolated publication, verified reads, garbage collection, cancellation, and stale multipart cleanup

Run an example with:

```sh
cargo run --example basic
```

Use a disposable bucket or restricted test prefix. Examples that create objects attempt to remove them, but interruption or service failure can leave test data behind.
