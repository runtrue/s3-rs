# sandboxd integration contract

`s3-wire` is independent of sandboxd. The compile-tested `examples/artifact_store.rs` demonstrates an adapter boundary suitable for a future sandboxd `ArtifactStore` provider without importing sandboxd types.

## Object layout

The example hashes each tenant identifier before placing it in a key prefix:

```text
tenants/<tenant-sha256>/blobs/sha256/<content-sha256>
tenants/<tenant-sha256>/manifests/sha256/<manifest-sha256>
```

Fixed-width lowercase digests prevent tenant strings from injecting delimiters or crossing prefixes. The application must still ensure that IAM and bucket policies restrict the runtime principal to its intended bucket and operations.

## Publication protocol

Payload bytes and manifest bytes are verified against their content identifiers before upload. Both use `If-None-Match: *`, which makes concurrent retries safe: an existing object is treated as an already-published immutable value rather than overwritten. Referenced payloads are created first and the manifest publication marker is created last, so a visible manifest does not precede its blobs.

Managed multipart currently has no destination precondition. For a large immutable payload, the adapter first checks existence and then requires the application to serialize concurrent writers for that digest. A stronger deployment can upload to a unique staging key and publish a small conditional manifest last.

## Reads and existence

Existence uses `HeadObject` and distinguishes `ErrorCategory::NotFound` from authorization, transport, or service errors. Download consumes `ResponseStream` chunk by chunk into an `AsyncWrite` destination while calculating the content-address SHA-256 digest. The adapter returns success only after the stream reaches EOF, the output is flushed, and the digest matches.

Applications writing a filesystem artifact should stream into a private temporary file and atomically rename it only after verification. Do not publish a partial destination when cancellation, timeout, length, or checksum validation fails.

## Garbage collection

Garbage collection lists only the tenant's blob prefix using `list_objects_v2_all` with an explicit maximum page count. It validates digest-shaped suffixes, compares them with the caller's live set, and deletes unreachable objects in batches no larger than 1,000. Per-object delete errors are treated as failure rather than silently counted as reclaimed data.

Manifest reachability and concurrent publication policy remain application responsibilities. Listing is not a transaction, so a production collector needs an age or generation barrier that prevents deletion of a blob being published concurrently.

## Multipart cleanup and cancellation

The adapter wraps managed multipart in a caller deadline and `CancellationToken`. Dropping the managed future signals the client-owned task, which cancels outstanding parts and attempts `AbortMultipartUpload` after an upload ID exists. A process crash can still leave an upload behind.

The example's stale-upload cleanup lists multipart uploads under the tenant prefix with a maximum page bound, selects entries older than a caller-provided cutoff, and aborts them individually. Choose a cutoff longer than the maximum legitimate upload duration to avoid racing active work.

## Configuration and credentials

The adapter accepts an injected `S3Client`, allowing sandboxd configuration to select:

- AWS or a validated custom S3-compatible endpoint;
- path-style or virtual-hosted addressing;
- region and bucket;
- connection, attempt, operation, and idle-body deadlines;
- response, listing, multipart concurrency, and multipart byte bounds; and
- any application-defined `CredentialsProvider`.

Use HTTPS for remote endpoints. Enable local-test HTTP only for loopback or a trusted diagnostic environment. Prefer a short-lived injected provider with least-privilege permissions over static process credentials.

Build the adapter example with:

```sh
cargo check --example artifact_store
```
