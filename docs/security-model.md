# Security model

## Protected assets

The client treats access keys, secret keys, session tokens, authorization headers, presigned URL query strings, upload IDs, and caller object data as sensitive. Request IDs, sanitized service error codes, object metadata, and object keys may still be operationally sensitive and should be logged according to the application's data policy.

## Trust boundaries

The calling application controls configuration, credential providers, object keys, upload bodies, and destinations for downloaded bytes. The endpoint and every S3 response are untrusted network input. DNS, public certificate roots, the Tokio runtime, process memory, and the host temporary directory are part of the deployment trust base.

## Transport and endpoint controls

- HTTPS with Rustls certificate and hostname verification is the default.
- Plain HTTP requires `allow_http_for_local_testing`; it should be restricted to loopback or a trusted diagnostic network.
- Endpoints must be absolute HTTP(S) URLs without user information, query strings, or fragments.
- The HTTP transport does not follow redirects. Controlled region correction applies only to validated standard AWS S3 HTTPS hosts and re-signs the new request.
- Virtual-hosted bucket names and path-style bucket components are validated before signing.
- Object keys are UTF-8 values of at most 1,024 encoded bytes. URL encoding preserves S3 key semantics, including repeated slashes and dot segments.

## Credential controls

Credentials use `secrecy::SecretString`; standard `Debug` and `Display` implementations do not expose secret keys, session tokens, presigned URLs, or upload IDs. `S3Error` retains only the source error type, not source text that could contain a URL, header, or response body. Credential refreshes are synchronized by the caching provider to prevent a refresh storm.

Presigned URLs are bearer credentials. `PresignedUrl` formatting is redacted and access requires `expose()` or `into_exposed()`. Applications must limit expiration, transport them over an authenticated channel, avoid logs and analytics, and consider them compromised after accidental disclosure.

The environment provider does not write credentials. The crate does not implement EC2, ECS, or web-identity metadata calls and does not persist credentials.

## Remote-input controls

- XML and error bodies have separate configurable byte limits; list-all operations require a caller-supplied maximum page count.
- XML containing DTD or entity declarations is rejected before parsing. External entity resolution and entity expansion are not supported.
- Header values, integer fields, timestamps, ranges, part numbers, checksums, ETags, pagination tokens, and upload IDs are validated before use.
- Download streams reject a body that exceeds or ends before its declared content length. A supported full-object SHA-256 response checksum is checked at end of stream.
- The crate forbids unsafe code and treats malformed service data as structured errors rather than panicking.

## Resource and availability controls

Configuration separately bounds connection, request-attempt, overall-operation, and idle-response-body time. Retry attempts, elapsed retry time, backoff, and `Retry-After` handling are bounded. Multipart part size, concurrency, and total in-flight bytes are validated before use. File-backed replay creates a disk snapshot, so operators must also place a quota and suitable permissions on the process temporary directory.

Managed multipart cancellation retains cleanup ownership after the caller drops the operation. Network partitions, process termination, or lost credentials can still prevent abort. Operators using multipart uploads should run bounded stale-upload reclamation with a conservative age cutoff.

## Integrity scope

Every signed payload has a SHA-256 payload hash. Replayable bodies are hashed by the client; one-shot bodies require a caller-supplied length and digest and are checked as they are transmitted. PutObject automatic additional checksum headers currently support SHA-256. Returned checksum headers are syntax-validated and exposed; only a full-object SHA-256 download checksum is recalculated by the response stream. Range responses are not compared with a full-object checksum.

SigV4 authenticates the request to the endpoint but does not make an unencrypted HTTP endpoint confidential. ETags are protocol metadata and are not treated as content hashes.

## Residual risks and operator responsibilities

- Protect environment variables, process memory, core dumps, temporary files, and explicitly exposed presigned URLs.
- Use least-privilege, short-lived credentials and bucket policies that constrain allowed prefixes and operations.
- Validate custom endpoint ownership and DNS before providing credentials.
- Apply an application-level digest or authenticated format when object content integrity must survive storage-side changes or when the service does not return a supported checksum.
- Serialize writers or use manifest-last publication when immutable names are uploaded through managed multipart, because that API has no destination precondition.
- Consume download streams to EOF before treating checksum verification as successful.
