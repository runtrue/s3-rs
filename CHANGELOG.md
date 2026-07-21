# Changelog

This project records user-visible changes in this file and follows semantic versioning.

## [Unreleased]

### Added

- Tokio-native S3 client with Rustls transport and internal SigV4 header and query signing.
- Typed configuration, endpoint validation, credential providers, structured errors, deadlines, retries, and bounded response parsing.
- Object CRUD, multi-object delete, copy, version-two listing, ranges, conditional operations, and presigned GET and PUT URLs.
- Replayable byte and immutable file-snapshot uploads, one-shot digest-supplied streams, streaming downloads, and bounded managed multipart orchestration.
- Primitive multipart operations and explicit upload, part, completion, abort, and cleanup state.
- Unit, property, deterministic mock-server, pinned MinIO, feature-gated AWS, fuzz, benchmark, and artifact-store adapter coverage.

### Limitations

- AWS chunked SigV4 streaming is not implemented.
- Automatic upload checksum calculation is limited to SHA-256.
- The managed multipart API has no destination precondition.
- The feature-gated real AWS compatibility suite has not yet been executed for this release.
