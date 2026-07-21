# S3 endpoint compatibility suite

The integration suite runs only when explicitly selected. Its local runner supports these official
images, each pinned to an immutable multi-platform manifest digest:

| Provider | Release | Digest |
| --- | --- | --- |
| MinIO | `RELEASE.2025-09-07T16-13-09Z` | `sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e` |
| RustFS | `1.0.0-beta.9` | `sha256:f75d0bca6ca322c4e59f7125f73dd9ab709b22f71396a42c95bce7f74c99e53b` |
| SeaweedFS | `4.40` | `sha256:52194fba4fecd0083c842158b3a902ba6e04a63619b2b0efcd08007bdb6a4602` |

Run it from the repository root:

```sh
./scripts/test-s3-compat.sh minio
./scripts/test-s3-compat.sh rustfs
./scripts/test-s3-compat.sh seaweedfs
```

The script requires Docker and `curl`, allocates a random loopback port, creates an isolated
container, and removes it on exit. Ordinary `cargo test` runs skip these tests.

To run against an already provisioned endpoint instead, export `S3_COMPAT_ENDPOINT`,
`S3_COMPAT_BUCKET`, `S3_COMPAT_ACCESS_KEY`, and `S3_COMPAT_SECRET_KEY`. Optionally set
`S3_COMPAT_PROVIDER` to label its isolated object prefix, then run:

```sh
cargo test --test s3_compat -- --ignored --nocapture
```
