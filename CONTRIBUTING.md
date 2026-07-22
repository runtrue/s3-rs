# Contributing

Contributions should preserve the crate's explicit resource bounds, replay rules, and secret-redaction guarantees.
Participation is governed by the [code of conduct](CODE_OF_CONDUCT.md). Support
and compatibility reports should follow [SUPPORT.md](SUPPORT.md).

## Development environment

The repository pins Rust 1.97.1 in `rust-toolchain.toml`. Install the pinned toolchain through rustup, then run:

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

The dry run validates packaging and must not be replaced with a publishing command.
Maintainers preparing a release should also follow the [release checklist](docs/releasing.md).

## Change requirements

- Keep public transport implementation types out of the API.
- Represent new limits in typed configuration and test their boundary cases.
- Classify retry behavior and prove body replayability before adding retries.
- Add request/response fixtures for protocol changes, including malformed and oversized cases.
- Add redaction tests for any type that can contain credentials, authorization data, upload IDs, signed URLs, or server-provided diagnostics.
- Avoid panics on remote input and keep `unsafe` code forbidden.
- Document public API changes and compatibility limits in `CHANGELOG.md` and the relevant document under `docs/`.

Dependencies should have a focused purpose, a maintained release, compatible licensing, and a feature set restricted to what the crate uses. Do not add a complete S3 client or invoke external programs at runtime.

## Integration tests

Run `./scripts/test-s3-compat.sh <minio|rustfs|seaweedfs>` for a pinned endpoint suite. It
requires Docker and `curl` and removes its isolated container on exit. Real AWS tests are opt-in
and must use a dedicated bucket or test prefix with least-privilege, short-lived credentials; see
[testing](docs/testing.md).

## Pull requests

Keep commits focused and describe externally visible behavior, tests, security effects, and resource-bound changes. Do not include credentials or signed URLs in commits, issue text, logs, fixtures, screenshots, or workflow artifacts.

Public API compatibility is checked against the latest published crate. During
the pre-1.0 line, an intentional breaking change still requires migration notes
and the version change implied by Cargo's compatibility rules. The MSRV may be
raised in a minor release, never silently in a patch release.
