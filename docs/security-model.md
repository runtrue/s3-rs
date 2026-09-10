# Security model

`s3-wire` protects request credentials and bounds untrusted service input, but it cannot secure the host, IAM policy, bucket policy, endpoint ownership, or data after an application explicitly exposes it.

## Protected data

The client treats these values as sensitive:

- access keys, secret keys, and session tokens;
- authorization headers and presigned URL query strings;
- multipart upload IDs; and
- caller object data; and
- caller-supplied request header values.

Request IDs, sanitized service codes, object metadata, bucket names, and object keys do not contain signing secrets by definition, but they may still be sensitive under an application's data policy.

## Trust boundaries

| Controlled by the application | Treated as untrusted | Part of the deployment trust base |
| --- | --- | --- |
| Configuration, credentials providers, keys, upload bodies, download destinations | Endpoint responses, headers, XML, redirects, lengths, checksums, pagination tokens | DNS, public certificate roots, Tokio, process memory, host filesystem, temporary directory |

The crate validates what crosses its API and network boundaries. Operators remain responsible for the trust base.

## Transport and endpoint controls

- HTTPS uses Rustls certificate and hostname verification by default.
- Plain HTTP requires `allow_http_for_local_testing` and should be limited to loopback or a trusted diagnostic network.
- Endpoints must be absolute HTTP(S) URLs without user information, query strings, or fragments.
- Redirects are not followed automatically.
- Region correction is limited to validated standard AWS S3 HTTPS hosts and is re-signed for the corrected endpoint.
- Bucket names are validated for the selected addressing style before signing.
- Object keys are UTF-8 values up to 1,024 encoded bytes; repeated slashes and dot segments are preserved as key data.

Custom endpoint ownership and DNS should be verified before credentials are provided.

## Credential and secret handling

Credentials use `secrecy::SecretString`. Standard `Debug` and `Display` output does not reveal secret keys, session tokens, presigned URLs, or upload IDs. `S3Error` retains source error types without source text that might contain a URL, header, or response body.

`CachedCredentialsProvider` serializes refreshes to avoid a refresh storm. The
environment provider reads credentials but does not write or persist them. The
opt-in `aws-credentials` feature can use profiles, credential processes,
web-identity endpoints, ECS endpoints, and EC2 IMDSv2. Enable it only in a
trusted workload environment, constrain metadata routing and IAM permissions,
and treat configured profile/process paths and endpoint variables as trusted
configuration. The default feature set makes none of those metadata calls.

Caller-supplied request headers are included in the SigV4 signature and may
contain secrets such as SSE-C keys. Request `Debug` implementations redact all
values from the public custom-header maps. The maps themselves remain directly
accessible, so applications are responsible for preventing their inspection,
serialization, or logging. Generated protocol headers and signing- or
transport-owned names cannot be replaced through a custom map.

Presigned URLs are bearer credentials. `PresignedUrl` is redacted by default and requires `expose()` or `into_exposed()` to access the complete URL. Applications should:

- use the shortest practical expiration;
- send the URL only over an authenticated channel;
- exclude it from logs, traces, analytics, and error messages; and
- treat it as compromised after accidental disclosure.

## Untrusted response handling

The client places explicit bounds around remote input:

- XML and service-error bodies have separate byte limits.
- `list_objects_v2_all` and multipart listing require caller-selected page bounds.
- DTD and entity declarations are rejected before XML deserialization.
- Header values, timestamps, integers, ranges, part numbers, checksums, ETags, pagination tokens, and upload IDs are validated before use.
- Download streams reject bodies that exceed or end before the declared content length.
- A returned full-object SHA-256 checksum is verified at EOF.
- Malformed remote input becomes a structured error rather than a panic.

The crate forbids unsafe code.

## Resource and availability controls

Client configuration and request-scoped options bound:

- connection timeout;
- request-attempt timeout;
- overall-operation timeout;
- idle-response-body timeout;
- retry attempts and elapsed retry time;
- a shared retry token quota across bucket handles;
- backoff and accepted `Retry-After` delay;
- XML and error response bytes;
- list pages;
- multipart part size and concurrency, with a derived in-flight byte bound.

File-backed replay creates a disk snapshot. Operators must provide enough temporary space and protect it with suitable permissions and quotas.

Managed multipart retains cleanup ownership after the public future is dropped. Abort can still fail during process termination, network partitions, or credential loss. Deployments using multipart should run bounded stale-upload cleanup with a conservative age cutoff.

## Integrity guarantees

Every signed payload uses a SHA-256 SigV4 payload hash:

- replayable bytes and file snapshots are hashed by the client;
- one-shot streams require a caller-supplied length and digest and are checked as transmitted;
- PutObject can add an S3 SHA-256 checksum;
- managed multipart can calculate CRC32, CRC32C, CRC64NVME, or SHA-256 per part;
- returned checksum headers are syntax-validated; and
- a returned full-object SHA-256 checksum is recalculated during streaming download.

Other returned checksum algorithms are exposed but not recalculated. Range responses are not compared with full-object checksums. ETags are protocol metadata and are not treated as content hashes.

SigV4 authenticates a request to the endpoint; it does not make a plain HTTP connection confidential.

## Operator checklist

- Use HTTPS for every remote endpoint.
- Prefer short-lived, least-privilege credentials and prefix-restricted bucket policies.
- Protect environment variables, process memory, core dumps, and temporary files.
- Never log authorization headers, custom request-header maps, upload IDs, or exposed presigned URLs.
- Consume downloads to EOF before accepting length or checksum verification.
- Add an application digest or authenticated format when storage-side integrity matters.
- Serialize immutable multipart writers or publish a conditional manifest last.
- Reclaim stale multipart uploads with bounded listing and a safe age cutoff.

Report suspected vulnerabilities through the process in [`SECURITY.md`](../SECURITY.md).
