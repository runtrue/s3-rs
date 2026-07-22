# Testing

The test strategy moves from fast, deterministic checks to opt-in service compatibility and performance runs. Ordinary `cargo test` does not require credentials, Docker, MinIO, or AWS.

## Fast local checks

Run the pull-request validation set:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
cargo semver-checks check-release --all-features
```

Before a release or packaging change, also run:

```sh
cargo package --locked
cargo publish --locked --dry-run
```

The publish dry run validates the package without uploading it. Do not substitute an actual `cargo publish` during routine validation.

CI also compiles every independently locked crate managed by Dependabot: the fuzz targets,
performance harness, and three size-comparison crates. The coverage check enforces a 75% line
coverage floor; its report and HTML output remain available as workflow artifacts.

## Test layers

| Layer | Location | Purpose |
| --- | --- | --- |
| Unit tests | `src/` | SigV4 vectors, validation, parsing, retries, credentials, streams, and state transitions |
| Property tests | `tests/properties.rs` and `src/` | Generated public values, endpoint URLs, protocol ordering, pagination, and redaction |
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

## Automated dependency updates

Dependabot checks Cargo and GitHub Actions dependencies every Monday. A Dependabot pull request is
approved and squash-merged only after every branch-protection check passes for its exact head
commit. A new commit cancels the pending approval run and must pass the complete gate again.

The required gate includes formatting, Clippy, rustdoc, all targets and features, auxiliary crates,
MinIO, RustFS, SeaweedFS, package and semver validation, dependency policy and review, RustSec,
secret scanning, and the enforced coverage floor. Major updates use the same gate; maintainers can
disable auto-merge on an individual pull request when a migration needs manual review.

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

The AWS suite is ignored by ordinary Cargo tests and gated by the `aws-compat`
feature:

```sh
cargo test --features aws-compat --test aws_compat -- --ignored --nocapture
```

It requires `S3_WIRE_AWS_BUCKET` and standard AWS credential variables. `AWS_REGION`, `AWS_ENDPOINT_URL`, and `S3_WIRE_AWS_ADDRESSING_STYLE` are optional; `tests/aws_compat.rs` defines their exact behavior.

Run the suite only against a dedicated bucket or restricted test prefix with
short-lived, least-privilege credentials. It creates a unique prefix and
attempts cleanup after failures. Never copy credentials or presigned URLs into
logs.

When repository variable `AWS_CONFORMANCE_REQUIRED` is `true`, the AWS
compatibility workflow runs daily on `main` and the release workflow calls it
with the exact release tag. Both paths use GitHub OIDC and the protected
`aws-compat` environment; no long-lived AWS access key is stored. With the gate
enabled, a release cannot enter its publish job unless that exact tag's called
compatibility job succeeds. When the variable is unset or not `true`, scheduled
and release-gate jobs are skipped and the release must not claim AWS validation.
Manual compatibility dispatch remains available. See [the release gate setup](releasing.md#repository-setup).

## Fuzzing

Install `cargo-fuzz`, enter the `fuzz/` directory, and run a named target:

```sh
cd fuzz
cargo fuzz run canonical_uri
```

Targets cover canonical URI and query construction, header canonicalization, S3 error XML, listing XML, multipart XML, and endpoint construction. Use a bounded run time. Convert every minimized crash into a normal regression fixture before closing the defect.

CI runs all targets nightly from checked-in seed corpora with bounded time,
per-input timeout, and RSS. Evolved corpora and crash artifacts are retained for
30 days; reviewed minimized inputs belong in `fuzz/corpus/<target>/`.

## Performance validation

Run the Criterion suite with:

```sh
cargo bench --features fuzzing --bench validation
```

The transfer and RSS harness is documented in [`tools/perf/README.md`](../tools/perf/README.md).
Performance workflows run weekly and remain manually dispatchable. Release-size
comparisons also run for relevant pull requests and reject a dirty source tree,
binary growth above the documented envelope, or unexpected dependency growth.
Shared-runner timings remain informational because host load is not stable.

Every retained result should record the source revision, dirty state, toolchain, features, command, service image digest, and measurement caveats. See [performance and size](performance.md) for the current baselines and interpretation rules.
