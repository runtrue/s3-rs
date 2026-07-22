# Releasing

Releases are published to crates.io from an existing `vMAJOR.MINOR.PATCH` tag by [`.github/workflows/release.yml`](../.github/workflows/release.yml).

## Before tagging

1. Confirm `Cargo.toml` contains the intended version and Rust version.
2. Move user-visible changes from `[Unreleased]` to a dated version section in `CHANGELOG.md`.
3. Confirm the README install command, compatibility statement, and limitations match the release.
4. Run the complete validation set:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --locked --all-targets -- -D warnings
   cargo clippy --locked --all-targets --all-features -- -D warnings
   cargo test --locked --all-targets
   cargo test --locked --all-targets --all-features
   RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
   cargo semver-checks check-release --all-features
   cargo package --locked
   cargo publish --locked --dry-run
   ```

   Public API compatibility is checked against the latest crates.io release.
   An intentional pre-1.0 breaking change requires migration notes and a
   version increment compatible with Cargo's semver interpretation.

5. Inspect the packaged file list:

   ```sh
   cargo package --locked --list
   ```

6. Run every pinned S3-compatible suite and record the results:

   ```sh
   ./scripts/test-s3-compat.sh minio
   ./scripts/test-s3-compat.sh rustfs
   ./scripts/test-s3-compat.sh seaweedfs
   ```

   When `AWS_CONFORMANCE_REQUIRED=true`, the release workflow runs the AWS
   suite itself against the exact tag commit; do not substitute an older
   scheduled result.

7. Confirm the `runtrue` organization-level `CRATES_APIKEY` Actions secret is visible to `runtrue/s3-rs`. Listing organization secrets requires organization-admin or fine-grained Actions-secret permission:

   ```sh
   gh secret list --org runtrue
   ```

8. Commit the release preparation and ensure the working tree is clean.

## Repository setup

The protected `aws-compat` GitHub environment must define:

- `AWS_ROLE_TO_ASSUME`: an IAM role trusted through GitHub OIDC and restricted
  to this repository, workflow environment, and expected refs;
- `S3_WIRE_AWS_BUCKET`: a dedicated compatibility bucket; and
- optional repository variable `AWS_COMPAT_REGION` (defaults to `us-east-1`).

Set repository variable `AWS_CONFORMANCE_REQUIRED` to exactly `true` only after
the role, bucket, and protected environment are operational. That enables daily
conformance and makes the exact-tag AWS job a mandatory predecessor of publish.
If the variable is unset or has any other value, scheduled and release AWS jobs
are skipped so repositories without AWS infrastructure can still publish; such
a release must be described as not AWS-validated.

The role should have only the bucket/prefix permissions exercised by
`tests/aws_compat.rs`, use a one-hour maximum session, and permit cleanup. No AWS
access-key secret is required. Environment reviewers may protect manual runs,
but unattended daily and release gates require deployment policy that allows
the intended refs without interactive approval.

## Publishing

Create an annotated tag on the release commit and push it:

```sh
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)
git tag -a "v$version" -m "Release $version"
git push origin "v$version"
```

The release workflow verifies that:

- the tag and `Cargo.toml` version agree;
- the tag points to a commit contained in `main`;
- when `AWS_CONFORMANCE_REQUIRED=true`, the AWS OIDC compatibility suite
  succeeds against that exact tag commit;
- public API compatibility passes against the latest published crate;
- formatting, Clippy, tests, rustdoc, packaging, and publish dry-run pass; and
- the `CRATES_APIKEY` organization secret is available to the repository for `cargo publish`.

The workflow can also be dispatched manually for an existing tag.

## After publishing

1. Confirm the version appears on [crates.io](https://crates.io/crates/s3-wire).
2. Confirm [docs.rs](https://docs.rs/s3-wire) built the crate successfully.
3. Verify the README badges and installation command.
4. Add release notes on GitHub from the corresponding changelog section if a GitHub release is desired.
5. Leave a fresh `[Unreleased]` section at the top of `CHANGELOG.md` for subsequent work.
