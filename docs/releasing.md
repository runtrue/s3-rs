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
   cargo package --locked
   cargo publish --locked --dry-run
   ```

   The public API remains intentionally unstable in the `0.1.x` line while it
   has a single owner. Add semantic-version compatibility checks to this gate
   once downstream compatibility becomes a release requirement.

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

   Run the opt-in AWS suite only when suitable least-privilege credentials and a disposable test prefix are available.

7. Confirm the `runtrue` organization-level `CRATES_APIKEY` Actions secret is visible to `runtrue/s3-rs`. Listing organization secrets requires organization-admin or fine-grained Actions-secret permission:

   ```sh
   gh secret list --org runtrue
   ```

8. Commit the release preparation and ensure the working tree is clean.

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
- formatting, Clippy, tests, rustdoc, packaging, and publish dry-run pass; and
- the `CRATES_APIKEY` organization secret is available to the repository for `cargo publish`.

The workflow can also be dispatched manually for an existing tag.

## After publishing

1. Confirm the version appears on [crates.io](https://crates.io/crates/s3-wire).
2. Confirm [docs.rs](https://docs.rs/s3-wire) built the crate successfully.
3. Verify the README badges and installation command.
4. Add release notes on GitHub from the corresponding changelog section if a GitHub release is desired.
5. Leave a fresh `[Unreleased]` section at the top of `CHANGELOG.md` for subsequent work.
