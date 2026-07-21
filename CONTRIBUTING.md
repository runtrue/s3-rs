# Contributing

Contributions should preserve the crate's explicit resource bounds, replay rules, and secret-redaction guarantees.

## Development environment

The repository pins Rust 1.97.1 in `rust-toolchain.toml`. Install the pinned toolchain through rustup, then run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo package
cargo publish --dry-run
```

The dry run validates packaging and must not be replaced with a publishing command.

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

Run `./scripts/test-minio.sh` for the pinned MinIO suite. It requires Docker and `curl` and removes its isolated container on exit. Real AWS tests are opt-in and must use a dedicated bucket or test prefix with least-privilege, short-lived credentials; see [testing](docs/testing.md).

## Pull requests

Keep commits focused and describe externally visible behavior, tests, security effects, and resource-bound changes. Do not include credentials or signed URLs in commits, issue text, logs, fixtures, screenshots, or workflow artifacts.
