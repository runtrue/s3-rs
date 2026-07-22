# sandboxd integration

`s3-wire` does not depend on sandboxd. The compile-tested [artifact store example](../examples/artifact_store.rs) shows the adapter boundary for a future sandboxd `ArtifactStore` provider without importing sandboxd types.

## Contract at a glance

The adapter demonstrates four application-level guarantees:

- tenant input cannot change the key hierarchy;
- content-addressed values are verified before upload and after download;
- immutable publication uses conditional writes and publishes manifests last; and
- listing, garbage collection, multipart work, and cleanup all have explicit bounds.

These are application policies built on `s3-wire` primitives, not behavior automatically imposed by the client.

## Object layout

Each tenant identifier is hashed before it becomes part of an object key:

```text
tenants/<tenant-sha256>/blobs/sha256/<content-sha256>
tenants/<tenant-sha256>/manifests/sha256/<manifest-sha256>
```

Fixed-width lowercase digests prevent tenant strings from injecting delimiters or escaping a prefix. IAM and bucket policies must still restrict the runtime principal to the intended bucket, prefix, and operations.

## Publication flow

The example publishes immutable content in this order:

1. Hash the payload and manifest locally.
2. Verify that each digest matches its content identifier.
3. Upload referenced payloads with `If-None-Match: *`.
4. Upload the manifest with the same precondition only after every payload succeeds.

An existing object is treated as an already-published immutable value rather than overwritten. Publishing the manifest last prevents a visible manifest from preceding its blobs.

Managed multipart does not currently support a destination `If-None-Match` condition. For a large immutable blob, the example checks whether the object exists and requires the application to serialize writers for that digest. A stronger design uploads to a unique staging key and conditionally publishes a small manifest last.

## Reads

Existence checks use `HeadObject` and treat only `ErrorCategory::NotFound` as absent. Authorization, transport, timeout, and service failures remain errors.

Downloads stream into an `AsyncWrite` destination while the adapter calculates the content-address SHA-256 digest. Success is returned only after:

- the body reaches EOF;
- the destination flushes; and
- the calculated digest matches.

A filesystem-backed artifact store should write to a private temporary file and atomically rename it after verification. It must not publish a partial file after cancellation, timeout, truncation, or checksum failure.

## Garbage collection

Garbage collection is deliberately bounded:

1. List only the tenant's blob prefix.
2. Stop after the configured maximum number of pages.
3. Reject suffixes that are not canonical digests.
4. Compare valid object digests with the caller's live set.
5. Delete unreachable values in batches of at most 1,000.
6. Treat every per-object delete error as a failure.

Listing is not transactional. A production collector needs an age or generation barrier so it cannot delete a blob being published concurrently. Manifest reachability and publication coordination remain application responsibilities.

## Multipart cancellation and cleanup

The adapter places the caller deadline in request-scoped `MultipartOptions` and
selects over the managed upload and a `CancellationToken`. Dropping the
operation signals the client-owned task, which cancels outstanding parts and
attempts `AbortMultipartUpload` after creation succeeds.

A crash, lost credential, or network partition can still prevent abort. The example therefore includes a bounded stale-upload pass that:

- lists multipart uploads under the tenant prefix;
- stops at a maximum page count;
- selects uploads older than a configured cutoff; and
- aborts each selected upload explicitly.

Choose a cutoff longer than the longest legitimate upload so cleanup does not race active work.

## Configuration checklist

The adapter accepts an injected `S3Client`. A sandboxd deployment can therefore choose:

- AWS or a validated S3-compatible endpoint;
- path-style or virtual-hosted addressing;
- region and bucket;
- connection, attempt, operation, and idle-body deadlines;
- response and pagination limits plus derived multipart buffer bounds; and
- an application-defined `CredentialsProvider`.

Use HTTPS for remote endpoints. Enable local-test HTTP only for loopback or a trusted diagnostic environment. Prefer short-lived, least-privilege credentials over static process credentials.

## Build the example

```sh
cargo check --example artifact_store
```
