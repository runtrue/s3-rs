# Changelog

This project records user-visible changes in this file and follows semantic versioning.

## [Unreleased]

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
