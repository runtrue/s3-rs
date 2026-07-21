# S3 compatibility

`s3-wire` focuses on object transfer rather than the full S3 control plane. It supports AWS-style SigV4 and both path-style and virtual-hosted requests, but compatibility claims are limited to operations and services that have actually been tested.

## Operation coverage

| Capability | Public API | Behavior |
| --- | --- | --- |
| Upload object | `put_object` | Bytes, immutable file snapshots, or a one-shot exact-length stream |
| Download object | `get_object` | Streaming body, ranges, conditions, version ID, and bounded timeouts |
| Inspect object | `head_object` | Metadata, conditions, and version ID |
| Delete object | `delete_object` | Version ID and ETag condition |
| Batch delete | `delete_objects` | 1–1,000 validated entries with per-object results |
| Server-side copy | `copy_object` | Source version and conditions; detects embedded errors in HTTP 200 responses |
| List objects | `list_objects_v2`, `list_objects_v2_all` | Prefix, delimiter, owner, page size, token-loop detection, and explicit page bound |
| Primitive multipart | create, upload part, complete, abort, list | Validated IDs, part order, ETags, checksums, and bounded response documents |
| Managed multipart | `multipart_upload` | Replayable bytes or files, bounded scheduling, and owned abort cleanup |
| Presigning | `presigned_get`, `presigned_put` | SigV4 query signing with a redacted URL wrapper |

Custom endpoint base paths and unusual valid object keys are encoded without treating keys as filesystem or browser paths. Repeated slashes and dot segments are preserved.

Multipart promotion is not automatic. Applications choose `put_object` or `multipart_upload` explicitly and may use the configured `multipart_threshold` as part of that policy.

## Addressing and endpoints

- `AddressingStyle::Path` places the bucket in the URL path.
- `AddressingStyle::VirtualHosted` places the bucket in the host.
- The configured endpoint may be a standard AWS region endpoint or a validated S3-compatible base URL.
- HTTPS is required unless `allow_http_for_local_testing` is enabled explicitly.
- Custom-endpoint redirects are not followed.

Virtual-hosted URL construction is covered by unit tests. The local S3-compatible suites use path style because virtual-hosted local testing would require wildcard DNS and certificates.

## Authentication and signing

The client implements SigV4 header signing and query presigning for S3, including region and service scope and optional session tokens. Retained deterministic tests include AWS documentation vectors.

Credentials may be static, loaded from the standard AWS environment variables, cached with expiration, or supplied through an application-defined async `CredentialsProvider`.

AWS chunked SigV4 streaming is not implemented. `ByteStream::from_stream` therefore requires the exact byte length and SHA-256 digest and produces a one-shot request that is not retried. Byte and file sources are replayable and may be retried when error classification and deadlines allow it.

## Checksums

Every signed payload has a SigV4 SHA-256 payload hash. Additional S3 checksum behavior is narrower:

| Direction | Behavior |
| --- | --- |
| PutObject | The client can calculate and send SHA-256 |
| UploadPart | Primitive requests accept validated caller-supplied CRC32, CRC32C, CRC64/NVME, SHA-1, or SHA-256 |
| GetObject | Returned checksum headers are validated and exposed; a full-object SHA-256 value is recalculated and checked at EOF |
| Range GET | A range body is not compared with a full-object checksum |

Selecting CRC32, CRC32C, CRC64/NVME, or SHA-1 for automatic PutObject calculation returns `UnsupportedOperation`.

## Tested services

### Deterministic local tests

The in-process suite covers wire shape, retry and replay decisions, response limits, malformed data, truncated bodies, checksum mismatch, pagination loops, multipart cleanup, timeouts, and redirect credential containment. It requires no external S3 service.

### S3-compatible services

The integration suite runs the same cases against MinIO `RELEASE.2025-09-07T16-13-09Z`, RustFS `1.0.0-beta.9`, and SeaweedFS `4.40`, each pinned by manifest digest. It covers:

- create, read, inspect, copy, list, and delete flows;
- conditional writes, ranges, metadata, empty objects, large objects, and unusual keys;
- primitive multipart completion and abort;
- managed multipart success and cleanup;
- concurrent operations; and
- presigned GET and PUT URLs.

Run a provider with:

```sh
./scripts/test-s3-compat.sh minio
./scripts/test-s3-compat.sh rustfs
./scripts/test-s3-compat.sh seaweedfs
```

Virtual-hosted URL construction has unit coverage but is not exercised by the local runners because it would require wildcard local DNS and certificate setup.

### AWS S3

An ignored, `aws-compat` feature-gated suite covers AWS CRUD, ranges, conditional creation, listing, primitive multipart, and presigned requests with cleanup. It has not yet been executed for this release.

Results from compatible servers are not proof of AWS compatibility.

## Outside the current API

The crate does not currently expose:

- bucket creation or administration;
- ACLs, bucket policies, lifecycle rules, or notifications;
- object version listing;
- S3 Select or object-lock administration;
- transfer acceleration or access points;
- multipart copy;
- application-managed encryption headers; or
- EC2, ECS, or web-identity credential fetching.

Unsupported service behavior is returned as `S3Error` rather than emulated through an unbounded fallback.
