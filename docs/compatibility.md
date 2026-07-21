# S3 compatibility

## Operation coverage

| Capability | Public API | Notes |
| --- | --- | --- |
| Upload object | `put_object` | Bytes, immutable file snapshots, or a one-shot exact-length/digest stream |
| Download object | `get_object` | Streaming body, ranges, conditions, version ID, bounded idle and operation timeouts |
| Inspect object | `head_object` | Metadata, conditions, version ID |
| Delete object | `delete_object` | Version ID and ETag condition |
| Batch delete | `delete_objects` | 1–1,000 validated entries with per-object results |
| Server-side copy | `copy_object` | Source version and source conditions; embedded HTTP-200 error detection |
| Object listing | `list_objects_v2`, `list_objects_v2_all` | Delimiter, prefix, owner, page size, token-loop detection, explicit page bound |
| Multipart | create, upload part, complete, abort, list | Validated upload IDs, part order, ETags, checksums, bounded response documents |
| Managed multipart | `multipart_upload` | Bytes or file source, bounded scheduling, owned abort cleanup |
| Presigning | `presigned_get`, `presigned_put` | SigV4 query signing; redacted URL wrapper |

Path-style and virtual-hosted addressing are supported. Custom endpoint base paths and unusual valid object keys are encoded without assigning filesystem or browser URL semantics to the key.

The client does not automatically promote PutObject to multipart. Applications may use the configured `multipart_threshold` as their selection policy and invoke `multipart_upload` explicitly.

## Authentication and signing

The client implements SigV4 header signing and query presigning for S3, including region/service scope and session tokens. The signing module is covered by retained deterministic vectors and AWS documentation vectors. Credentials may come from static, environment, cached, or application-defined async providers.

AWS chunked SigV4 streaming is not implemented. A non-seekable `ByteStream::from_stream` must include the exact byte length and SHA-256 digest, and the resulting one-shot request is not retried. Byte and file sources are replayable and may be retried when the operation classification permits.

## Checksum behavior

All request payloads use a SHA-256 SigV4 payload hash. PutObject can add the S3 SHA-256 checksum header when `ChecksumAlgorithm::Sha256` is selected. Selecting CRC32, CRC32C, CRC64/NVME, or SHA-1 for automatic PutObject calculation returns `UnsupportedOperation`.

Primitive upload-part requests accept caller-provided, base64-encoded CRC32, CRC32C, CRC64/NVME, SHA-1, or SHA-256 values and validate their encoded length and padding. Returned checksum headers are syntax-validated and exposed. Streaming GET verifies a returned full-object SHA-256 checksum at EOF; it does not recalculate other algorithms or compare range bodies against a full-object checksum.

## Tested services

The deterministic in-process suite covers wire behavior, retry and replay decisions, response limits, malformed data, body truncation, checksum mismatch, pagination loops, multipart cleanup, timeouts, and redirect credential containment.

The local integration suite uses MinIO `RELEASE.2025-09-07T16-13-09Z`, pinned by manifest digest. It covers CRUD, path-style requests, conditional writes, ranges, metadata, pagination, multipart completion and abort, managed cleanup, concurrent operations, presigned URLs, empty objects, large transfers, and unusual keys. Virtual-hosted URL construction has unit coverage but is not exercised by the local MinIO runner because it would require wildcard local DNS and certificate setup. Run the suite with `./scripts/test-minio.sh`.

An ignored, `aws-compat` feature-gated suite covers AWS CRUD, ranges, conditional creation, listing, primitive multipart, and presigned requests with cleanup. It has not yet been executed for this release. The MinIO result must not be interpreted as proof of AWS compatibility.

## Unsupported API families

The crate does not expose bucket creation or administration, ACLs, bucket policies, lifecycle rules, notification configuration, object version listing, S3 Select, object lock administration, transfer acceleration, access points, multipart copy, or application-managed encryption headers. Unknown or unsupported service behavior is reported through `S3Error` rather than emulated with an unbounded fallback.
