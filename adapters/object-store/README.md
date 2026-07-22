# Apache `object_store` adapter plan

The proposed adapter will be a separately versioned, separately publishable crate. It will depend on `s3-wire` and Apache `object_store`, but neither dependency nor its transitive multi-cloud surface will be added to the core crate.

Before implementation, the adapter needs:

1. stable streaming upload, download, range, multipart, copy, listing, and conditional-write contracts in `s3-wire`;
2. an explicit mapping for `object_store::Error` that preserves sanitized S3 status, service code, request ID, retry exhaustion, and uncertain mutation outcomes;
3. paginator streams with bounded retained pages;
4. cancellation tests proving multipart cleanup ownership; and
5. a compatibility matrix against the minimum supported `object_store` version and its current release.

The adapter should use a caller-provided `S3Client` or shared transport, avoid its own credential chain, preserve `s3-wire` deadlines and retry limits, and document semantic differences instead of silently emulating unsupported behavior. It will have its own `Cargo.toml`, lockfile, CI, semver policy, changelog, and release cadence. This directory intentionally contains no manifest until those API prerequisites are complete, so it cannot accidentally enter the root package or CI dependency graph.
