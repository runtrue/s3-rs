# Support policy

## Supported releases

The latest published `0.2.x` release and the `main` branch receive correctness and security fixes. Older pre-1.0 releases are supported only when a maintainer explicitly announces a maintained release line. Security fixes may be released immediately as a patch version.

Rust 1.97.1 is the minimum supported Rust version (MSRV). CI tests that exact toolchain. The MSRV may be raised in a minor release before 1.0, with the change called out in the changelog; patch releases do not intentionally raise it.

## Getting help

Before opening an issue:

1. Check the [compatibility guide](docs/compatibility.md), documented non-goals, and current release notes.
2. Reproduce with the latest release or `main` and the smallest possible operation.
3. Capture the client version, Rust version, target, provider and version, region, addressing style, enabled features, stable error category, HTTP status, service error code, and request ID.
4. Remove credentials, authorization headers, session tokens, signed URL query strings, private endpoint and bucket names, and object contents.

Use the bug form for client defects, the compatibility form for provider-specific behavior, and the feature form for proposals. Community support is best effort; no response-time or service-level commitment is implied.

Suspected vulnerabilities must use [private vulnerability reporting](https://github.com/runtrue/s3-rs/security/advisories/new), not a public issue. See [SECURITY.md](SECURITY.md).

## Scope

The core project supports a lightweight async S3 data-plane client. Bucket administration and broad AWS control-plane parity are not project goals. Ecosystem integrations should normally live in separately versioned adapter crates so their dependency trees do not change the core client's footprint.
