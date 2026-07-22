# Roadmap

The project aims to be the smallest production-grade Rust S3 data-plane client with explicit resource bounds, reliable streaming, durable recovery, protocol correctness, and evidence-backed compatibility. This roadmap is directional; an issue and reviewed design still precede public API work.

## Correctness and compatibility

- Complete operation-specific handling for embedded S3 errors, retries, metadata, encoded listings, conditions, and uncertain mutating outcomes.
- Make the real-AWS OIDC suite a release gate and continuously retain redacted evidence for AWS, MinIO, RustFS, and SeaweedFS.
- Expand unusual-key, region, addressing, checksum, session-credential, and multipart conformance cases.

## Production transfers

- Durable resumable multipart upload with source identity and bounded checkpoints.
- Unknown-length multipart streaming with explicit memory and temporary-storage ceilings.
- Atomic resumable path downloads that cannot mix object generations.
- Modern full-object and composite checksum calculation and verification.
- Optional workload credential, region, partition, encryption, and account-safety integrations.

## Ecosystem and evidence

- Lazy bounded paginator streams, shared transports, sanitized observers, and uncertain-outcome metadata.
- A separately versioned Apache `object_store` adapter after the core transfer contracts stabilize; see [the adapter plan](adapters/object-store/README.md).
- Reproducible comparison workloads covering binary size, dependencies, build cost, throughput, latency, memory, temporary disk, and recovery behavior.
- Continuous semver analysis, fuzzing, concurrency testing, and retained provider reports.

## Non-goals for the core crate

- Complete AWS SDK or S3 control-plane parity.
- Bucket policy, lifecycle, notification, replication, analytics, inventory, or ACL administration.
- A multi-cloud storage abstraction.
- Convenience behavior with hidden or unbounded memory, disk, concurrency, retries, or deadlines.

Requests that conflict with these boundaries may be better served by the official AWS SDK, a control-plane companion crate, or an ecosystem adapter.
