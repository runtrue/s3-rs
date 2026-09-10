# Changelog

This project records user-visible changes in this file and follows semantic versioning.

## [Unreleased]

## [0.3.1] - 2026-09-10

### Changed

- Added `s3-rs` to the crates.io keywords, replacing `sigv4` within the
  five-keyword limit. Runtime behavior and public APIs are unchanged.

## [0.3.0] - 2026-09-10

### Added

- Every ordinary signed operation accepts caller-supplied request headers,
  preserves repeated values, and signs them before retries or AWS region
  correction. Managed multipart uploads provide separate create, upload-part,
  complete, and abort header maps.

### Changed

- Generated, signing-owned, and transport-owned header collisions are rejected
  instead of silently replacing either value. Custom header values are redacted
  from request `Debug` output.

### Fixed

- Updated XML root-name handling for `quick-xml` 0.42 and refreshed dependencies,
  including the patched `h2` 0.4.16 release.

### Migration

- Request types expose new `headers` fields. Code constructing request structs
  directly must initialize these fields or use the provided constructors.
- Multi-object delete entries now use `DeleteObjectIdentifier` rather than
  `DeleteObjectRequest`; request-level headers belong on `DeleteObjectsRequest`.
- URL-only presigning does not accept custom headers. A future presigned-request
  API would need to return required headers together with the URL.

## [0.2.0] - 2026-07-22

### Added

- Atomic verified downloads to a destination path, shared-transport bucket
  handles, sanitized request lifecycle observers, and public retry attempt and
  stop-reason metadata.
- Optional `aws-credentials` integration for the standard renewable AWS
  credential and region chains while keeping the default dependency graph
  independent of the AWS SDK runtime.
- Partition-aware standard, dual-stack, FIPS, and FIPS dual-stack AWS endpoint
  construction, plus presigning for HEAD, DELETE, and multipart operations.
- Typed `ListParts` and `UploadPartCopy` APIs and local CRC32, CRC32C,
  CRC64NVME, and SHA-256 checksum calculation for managed multipart parts.

### Changed

- Copy and multipart-completion operations now retry retryable S3 errors embedded
  in HTTP 200 responses under the same replay, attempt, and operation bounds as
  ordinary service failures.
- Copy metadata behavior is explicit: callers either preserve all source
  metadata or provide the complete replacement media type and user metadata.
- Object listings request URL encoding, decode keys and common prefixes exactly
  once, and expose owner and checksum metadata returned by S3.
- Retry handling recognizes additional AWS transient and throttling errors,
  applies throttling-specific backoff, honors both standard and Amazon retry
  delay headers, and preserves AWS endpoint variants during region correction.
- Multipart support now includes bounded `ListParts` and in-progress-upload
  pagination, `UploadPartCopy`, typed checksum aggregation, and automatic
  CRC32, CRC32C, CRC64NVME, or SHA-256 checksums for managed upload parts.

### Migration

- `CopyObjectRequest` now uses `CopyMetadataDirective` instead of independent
  content-type and metadata fields. `CopyObjectRequest::new` preserves all
  source metadata; select `Replace` only with the complete replacement set.
- Listing and multipart response models expose new owner and checksum fields.
  Code that constructs or exhaustively destructures those public models must
  initialize or ignore the new fields.

## [0.1.1] - 2026-07-22

### Changed

- Managed multipart uploads now use request-scoped `MultipartOptions` with a
  derived buffer bound, one transfer deadline, and a separate cleanup deadline.
- HTTP/1.1 is the lightweight default transport; HTTP/2 is available through
  the `http2` feature.
- Request and response bodies use concrete stream types, implementation modules
  and endpoint URLs are private, and supported types remain available from the
  crate root.
- File-backed PUT and multipart sources share one immutable snapshot
  implementation, and successful multipart responses are drained under a small
  bound for connection reuse.
- Operation deadlines now include upload-body preparation, in-memory hashing is
  cooperative, multipart cleanup quiesces transmitted part requests before
  aborting, and service status is exposed as a crate-independent numeric code.

## [0.1.0] - 2026-07-21

### Added

- Tokio-native S3 client with Rustls transport and internal SigV4 header and query signing.
- Typed configuration, endpoint validation, credential providers, structured errors, deadlines, retries, and bounded response parsing.
- Object CRUD, multi-object delete, copy, version-two listing, ranges, conditional operations, and presigned GET and PUT URLs.
- Replayable byte and immutable file-snapshot uploads, one-shot digest-supplied streams, streaming downloads, and bounded managed multipart orchestration.
- Primitive multipart operations and explicit upload, part, completion, abort, and cleanup state.
- Unit, property, deterministic mock-server, pinned MinIO/RustFS/SeaweedFS integration, feature-gated AWS, fuzz, benchmark, and artifact-store adapter coverage.

### Limitations

- AWS chunked SigV4 streaming is not implemented.
- Automatic upload checksum calculation is limited to SHA-256.
- The managed multipart API has no destination precondition.
- The feature-gated real AWS compatibility suite has not yet been executed for this release.
