# Architecture

`s3-wire` keeps the public API small and the wire implementation private. Applications work with typed configuration, operations, streams, and errors; HTTP and XML types do not leak into their code.

## At a glance

| Layer | Responsibility |
| --- | --- |
| `config` | Validate endpoints, addressing, credentials, timeouts, retries, and response limits |
| `operation` | Define public requests, responses, object keys, conditions, checksums, listings, and multipart state |
| `endpoint` | Build exact path-style or virtual-hosted request targets without applying filesystem semantics to keys |
| `signing` | Produce SigV4 canonical requests, authorization headers, and presigned query strings |
| `transport` | Send HTTP/1.1 requests, optionally negotiate HTTP/2, and adapt Rustls-protected response bodies into streams |
| `protocol` | Serialize and parse bounded S3 XML documents while rejecting DTD and entity declarations |
| `retry` | Classify failures and calculate bounded backoff with jitter and `Retry-After` support |
| `stream` | Model replayable and one-shot uploads plus backpressured, integrity-checked downloads |
| `client` | Join the layers, enforce deadlines, classify responses, and orchestrate retries and multipart uploads |

These implementation modules are private. Their intentionally supported types
are re-exported from the crate root so applications have one compact API path.

## Request flow

Every operation follows the same path:

1. Configuration and request types reject invalid local input.
2. `endpoint` builds the exact URL for the configured addressing style.
3. The credential provider supplies non-expired credentials.
4. `signing` hashes the payload description and signs the method, target, query, and selected headers.
5. `transport` sends the request, using Rustls for HTTPS.
6. `client` parses success metadata or a bounded S3 error document into a structured result.
7. A retry happens only when the failure classification, deadline, attempt budget, and body replayability all allow it. Each attempt is signed again.

The transport never follows redirects automatically. Region correction is accepted only for standard AWS S3 HTTPS endpoints. The client validates the returned region, reconstructs the endpoint, and re-signs the request. Redirects from custom endpoints are returned as errors so signed headers are not forwarded elsewhere.

## Upload ownership

The upload source determines whether a request can be replayed:

| Source | Ownership and retry behavior |
| --- | --- |
| `ByteStream::from_bytes` | Retains and hashes the bytes; every attempt reads the same value |
| `ByteStream::from_path` | Hashes a regular file into a private, read-only temporary snapshot; every attempt reopens that snapshot |
| `ByteStream::from_stream` | Takes an exact length and SHA-256 digest; the stream is consumed once and is never retried |

The client does not silently buffer a one-shot stream. File replay uses disk rather than memory, so operators must size and protect the process temporary directory.

## Download ownership

`ResponseStream` yields chunks only as the caller consumes them. While streaming, it enforces:

- declared content length;
- idle-body timeout;
- overall operation deadline; and
- a returned full-object SHA-256 checksum, when present.

Dropping the response stops body work; there is no detached download task. The caller must reach EOF before treating length or checksum verification as complete.

## Multipart ownership

Managed multipart accepts replayable bytes or a file snapshot. Each request owns
validated `MultipartOptions`: part size, concurrency, an end-to-end transfer
deadline, and a separate cleanup deadline. The maximum retained part bytes are
derived from part size times concurrency.

After S3 creates an upload, a client-owned task retains the upload ID. If the public future is dropped, that task stops scheduling parts, gives transmitted requests a bounded opportunity to settle, and attempts `AbortMultipartUpload` within the cleanup deadline. A normal failure waits for cleanup; if requests cannot be quiesced or abort fails, `S3Error::cleanup_failure()` preserves the cleanup error beside the primary error.

Multipart selection remains application policy; `put_object` never changes
operation type automatically.

## Dependency choices

- Tokio provides the runtime, timers, synchronization, cancellation integration, and async file I/O.
- Hyper and `hyper-util` provide HTTP/1.1 by default without becoming part of
  the public API. The `http2` feature adds HTTP/2 negotiation.
- Rustls and `hyper-rustls` provide TLS with WebPKI roots; certificate verification cannot be disabled.
- `quick-xml` and Serde handle the focused XML documents used by supported operations.
- `sha2`, `hmac`, `md-5`, `base64`, `subtle`, and `zeroize` cover signing, integrity fields, constant-time comparisons, and key-material cleanup.
- `secrecy` makes credential and presigned-URL exposure explicit.
- `tempfile` owns replay snapshots and deletes them on drop.

Unused default features are disabled where practical. Comparison clients live under `comparisons/` and are not dependencies of the published crate.

## Extending the client

New operations should use typed public requests and responses, then reuse the existing endpoint, signing, deadline, response-limit, error, and retry paths. New credential sources can implement the transport-independent `CredentialsProvider` trait without changing `S3Client`.

Changes should preserve three invariants:

- remote input is bounded and never trusted;
- a retry is impossible unless the request body is provably replayable; and
- values containing credentials or signing material remain redacted by default.
