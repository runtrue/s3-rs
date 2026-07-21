# Architecture

## Scope

`s3-wire` is one reusable library crate. Its public API describes S3 operations, configuration, credentials, object keys, streams, retries, and errors. HTTP and XML implementation types remain internal so applications are not coupled to the current transport.

## Module boundaries

- `client` maps typed operations to signed HTTP requests, enforces deadlines, classifies responses, and owns retry and multipart orchestration.
- `config` validates endpoints, addressing, timeouts, response limits, retry policy, and multipart memory bounds before a request is sent.
- `credentials` defines the async provider interface and implements static, environment, and expiration-aware cached providers.
- `endpoint` constructs exact request targets. Object-key bytes have no filesystem semantics; repeated slashes and dot segments are preserved and encoded for S3.
- `operation` contains public request, response, object-key, checksum, condition, listing, and multipart state types.
- `signing` produces SigV4 canonical requests, header signatures, and presigned query strings independently of the transport.
- `protocol` serializes and parses bounded S3 XML documents. Remote XML is rejected before deserialization if it declares a DTD or entity.
- `retry` calculates bounded exponential backoff with jitter and honors valid `Retry-After` values within the operation deadline.
- `stream` implements replay-aware upload bodies and backpressured response bodies with length, timeout, and supported checksum verification.
- `transport` owns the Hyper client, Rustls configuration, connection timeout, and HTTP response stream adaptation.

## Request lifecycle

1. Configuration and operation types validate local input.
2. The endpoint module builds an exact target for path-style or virtual-hosted addressing.
3. A credential provider supplies non-expired credentials.
4. The signing module hashes the payload description and signs the method, target, query, and selected headers.
5. The Hyper transport sends the request through Rustls for HTTPS endpoints.
6. The client parses success metadata or a bounded S3 error document and assigns a structured error category.
7. A retry occurs only when classification, elapsed time, attempt count, and body replayability all permit it. Each attempt is re-signed.

The transport does not automatically follow redirects. A region correction is accepted only for standard HTTPS AWS S3 endpoints and is reconstructed from a validated region response before the request is re-signed. Custom-endpoint redirects are returned as errors without forwarding signed headers.

## Transfer and ownership model

`ByteStream::from_bytes` hashes retained bytes and can replay them. `ByteStream::from_path` copies a regular file into a read-only private temporary file while hashing, then opens that snapshot for each attempt. `ByteStream::from_stream` receives an exact length and SHA-256 digest from the caller and can be consumed once; the client does not buffer it or retry it.

`ResponseStream` yields chunks with backpressure. It enforces declared content length, idle-body timeout, overall deadline, and a returned full-object SHA-256 checksum when present. Dropping a response stops further body work because there is no detached download task.

Managed multipart sources are replayable bytes or immutable file snapshots. Part size, concurrency, and total in-flight part bytes are validated together. A client-owned task keeps the upload ID once creation succeeds. Dropping the public future signals cancellation; the owned task cancels outstanding parts and attempts abort. Normal failures wait for abort and preserve an abort failure as `S3Error::cleanup_failure`.

Multipart selection is a caller policy. The configuration retains a validated `multipart_threshold`, but `put_object` does not automatically change operation type; callers invoke `multipart_upload` explicitly.

## Dependency decisions

- Tokio supplies the async runtime, file I/O, timers, synchronization, and cancellation integration used by applications in scope.
- Hyper and `hyper-util` provide HTTP/1.1 and HTTP/2 without exposing their types publicly. Redirect behavior remains under client control.
- Rustls and `hyper-rustls` provide TLS with WebPKI roots and no native TLS dependency. Certificate verification is not configurable off.
- `quick-xml` with Serde supports focused S3 documents; a separate pre-scan rejects DTD and entity syntax and every remote document has a configured byte bound.
- `sha2`, `hmac`, `md-5`, `base64`, `subtle`, and `zeroize` cover signing, protocol integrity fields, constant-time comparisons, and key-material cleanup without a general cryptographic framework.
- `secrecy` makes credential and presigned-URL exposure explicit and redacts standard formatting.
- `tempfile` provides owned disk snapshots for replayable file transfers and cleanup on drop.

Default features are disabled where the selected dependency would otherwise bring unused TLS or runtime implementations. Comparison clients are isolated under `comparisons/` and are not dependencies of the published crate.

## Public API evolution

The credential provider trait is transport-independent so metadata, workload-identity, or external refresh implementations can be added by applications without changing `S3Client`. Additional S3 operations should be represented by typed requests and outputs and should reuse the same endpoint, signing, limit, deadline, and error paths.
