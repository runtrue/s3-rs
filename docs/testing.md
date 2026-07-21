# Testing

## Fast local checks

Run the same core checks used for pull requests:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo package
cargo publish --dry-run
```

`cargo publish --dry-run` validates the package only. Do not run an actual publish command as part of validation.

## Test layers

Unit tests cover official and retained SigV4 vectors, encoding, canonical ordering, header normalization, credential caching and redaction, retry policy, endpoints, bounded XML, error mapping, multipart state, body integrity, timeouts, and pagination validation.

Property tests are in `tests/properties.rs`. They exercise object-key and URL encoding, canonical ordering, header normalization, pagination behavior, and credential redaction over generated inputs.

`tests/mock_client.rs` and its in-process support server exercise signed request shape, retry counts, replayability, throttling, oversized and malformed XML, truncated bodies, delayed body chunks, download checksum verification, multipart failure and cleanup, pagination-token loops, conditional failures, and redirect-based credential forwarding attempts. These tests require no external service.

Fuzz targets live under `fuzz/fuzz_targets` for canonical URI, canonical query, header canonicalization, S3 error XML, listing XML, multipart XML, and endpoint construction. Install `cargo-fuzz`, then run a named target from `fuzz/`, for example:

```sh
cd fuzz
cargo fuzz run canonical_uri
```

Use bounded run times and retain any minimized regression input as a normal test fixture before closing a parser or signing defect.

## S3 endpoint integration

The reproducible local command is:

```sh
./scripts/test-s3-compat.sh minio
./scripts/test-s3-compat.sh rustfs
./scripts/test-s3-compat.sh seaweedfs
```

The runner requires Docker and `curl`. It starts the selected provider from an immutable image
digest on a random loopback port, creates an isolated bucket, runs the ignored integration suite,
and removes the container on exit. CI runs the same cases against MinIO, RustFS, and SeaweedFS.
See `tests/s3_compat.md` for exact releases and digests.

To use an already provisioned S3-compatible instance:

```sh
export S3_COMPAT_ENDPOINT=http://127.0.0.1:9000
export S3_COMPAT_BUCKET=s3-wire-test
export S3_COMPAT_ACCESS_KEY=local-test-access
export S3_COMPAT_SECRET_KEY=local-test-secret
cargo test --test s3_compat -- --ignored --nocapture
```

Use disposable credentials and a disposable bucket. Plain HTTP is appropriate only for the local-test mode exercised here.

## AWS compatibility

The AWS suite is manual, ignored, and feature-gated. It requires `S3_WIRE_AWS_BUCKET`, standard AWS credential variables, and optionally `AWS_REGION`, `AWS_ENDPOINT_URL`, and `S3_WIRE_AWS_ADDRESSING_STYLE` as defined in `tests/aws_compat.rs`.

```sh
cargo test --features aws-compat --test aws_compat -- --ignored --nocapture
```

Run it only with a dedicated bucket or restricted test prefix and short-lived least-privilege credentials. The test creates a unique prefix and attempts cleanup even after failures. Never paste its credentials or presigned URLs into logs. This suite has not yet been executed for the current release.

## Performance validation

Microbenchmarks run with:

```sh
cargo bench --features fuzzing --bench validation
```

The MinIO transfer and RSS harness is under `tools/perf`; its README documents inputs and output. Performance workflows are manual because shared-runner and local-container measurements are sensitive to host load. Retained results must include the revision, dirty state, toolchain, dependency features, commands, service image digest, and measurement caveats.
