# MinIO compatibility suite

The integration suite runs only when explicitly selected. Its local runner starts the official
MinIO `RELEASE.2025-09-07T16-13-09Z` image pinned to the immutable multi-platform manifest digest
`sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e`.

Run it from the repository root:

```sh
./scripts/test-minio.sh
```

The script requires Docker and `curl`, allocates a random loopback port, creates an isolated
container, and removes it on exit. Ordinary `cargo test` runs skip these tests.

To run against an already provisioned MinIO endpoint instead, export `MINIO_S3_ENDPOINT`,
`MINIO_S3_BUCKET`, `MINIO_ROOT_USER`, and `MINIO_ROOT_PASSWORD`, then run:

```sh
cargo test --test minio -- --ignored --nocapture
```
