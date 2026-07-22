# Testing

The test strategy moves from fast, deterministic checks to opt-in service compatibility and performance runs. Ordinary `cargo test` does not require credentials, Docker, MinIO, or AWS.

## Fast local checks

Run the pull-request validation set:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
```

Before a release or packaging change, also run:

```sh
cargo package --locked
cargo publish --locked --dry-run
```

The publish dry run validates the package without uploading it. Do not substitute an actual `cargo publish` during routine validation.

## Test layers

| Layer | Location | Purpose |
| --- | --- | --- |
| Unit tests | `src/` | SigV4 vectors, validation, parsing, retries, credentials, streams, and state transitions |
| Property tests | `tests/properties.rs` | Generated keys, URLs, canonical ordering, headers, pagination, and redaction |
| In-process integration | `tests/mock_client.rs` | Exact wire behavior, errors, retries, timeouts, checksums, multipart cleanup, and redirect containment |
| S3-compatible integration | `tests/s3_compat.rs` | End-to-end behavior against pinned MinIO, RustFS, and SeaweedFS releases |
| AWS compatibility | `tests/aws_compat.rs` | Opt-in behavior against a configured AWS bucket |
| Fuzz targets | `fuzz/fuzz_targets/` | URI, query, headers, XML, and endpoint parsing |
| Benchmarks | `benches/` and `tools/perf/` | CPU regression points, transfer behavior, and sampled RSS |

## What deterministic tests cover

Unit and in-process integration tests exercise:

- AWS documentation and retained SigV4 vectors;
- object-key and URL encoding;
- canonical query ordering and header normalization;
- credential caching and redaction;
- endpoint, range, part, checksum, and pagination validation;
- bounded success and error XML;
- response classification and retry decisions;
- replayable versus one-shot bodies;
- malformed, oversized, delayed, and truncated responses;
- download length and checksum verification;
- multipart failure, cancellation, and cleanup;
- pagination-token loops; and
- attempts to forward credentials through redirects.

These tests require no external service and should be the first place to add a protocol regression.

## S3 endpoint integration

Run the reproducible local suites with:

```sh
./scripts/test-s3-compat.sh minio
./scripts/test-s3-compat.sh rustfs
./scripts/test-s3-compat.sh seaweedfs
```

The runner requires Docker and `curl`. It starts the selected provider from an immutable image digest on a random loopback port, creates an isolated bucket, runs the ignored integration suite, and removes the container on exit. CI runs the same cases against MinIO, RustFS, and SeaweedFS. [`tests/s3_compat.md`](../tests/s3_compat.md) records the exact releases and digests.

To use an already provisioned S3-compatible instance:

```sh
export S3_COMPAT_ENDPOINT=http://127.0.0.1:9000
export S3_COMPAT_BUCKET=s3-wire-test
export S3_COMPAT_ACCESS_KEY=local-test-access
export S3_COMPAT_SECRET_KEY=local-test-secret
cargo test --test s3_compat -- --ignored --nocapture
```

Use disposable credentials and a disposable bucket. Plain HTTP is appropriate only for the local-test mode shown here.

## AWS compatibility

The AWS suite is manual, ignored, and gated by the `aws-compat` feature:

```sh
cargo test --features aws-compat --test aws_compat -- --ignored --nocapture
```

It requires `S3_WIRE_AWS_BUCKET` and standard AWS credential variables. `AWS_REGION`, `AWS_ENDPOINT_URL`, and `S3_WIRE_AWS_ADDRESSING_STYLE` are optional; `tests/aws_compat.rs` defines their exact behavior.

Run the suite only against a dedicated bucket or restricted test prefix with short-lived, least-privilege credentials. It creates a unique prefix and attempts cleanup after failures. Never copy credentials or presigned URLs into logs. This suite has not yet been executed for the current release.

## Fuzzing

Install `cargo-fuzz`, enter the `fuzz/` directory, and run a named target:

```sh
cd fuzz
cargo fuzz run canonical_uri
```

Targets cover canonical URI and query construction, header canonicalization, S3 error XML, listing XML, multipart XML, and endpoint construction. Use a bounded run time. Convert every minimized crash into a normal regression fixture before closing the defect.

## Performance validation

Run the Criterion suite with:

```sh
cargo bench --features fuzzing --bench validation
```

The transfer and RSS harness is documented in [`tools/perf/README.md`](../tools/perf/README.md). Performance workflows are manual because shared runners and local containers are sensitive to host load.

Every retained result should record the source revision, dirty state, toolchain, features, command, service image digest, and measurement caveats. See [performance and size](performance.md) for the current baselines and interpretation rules.
