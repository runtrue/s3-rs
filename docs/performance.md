# Performance and size

The measurements in this repository are reproducible regression points, not general claims about S3 throughput or capacity. Each result belongs to a specific revision, toolchain, machine, dependency set, and service configuration.

## Measurement sets

| Report | Measures | Does not measure |
| --- | --- | --- |
| [Microbenchmark baseline](../measurements/microbench-2026-07-21.md) | Canonicalization, signing, XML, client construction, body preparation, multipart scheduling | Network, TLS, disk, or object-store latency |
| [Local MinIO transfer run](../measurements/results/minio-local.md) | PUT, streaming GET, managed multipart, disk-backed multipart, sampled process RSS | Remote-network behavior or production capacity |
| [Release-build comparison](../comparisons/size/REPORT.md) | Dependency packages, clean build, incremental rebuild, stripped minimal binary | API completeness, correctness, runtime throughput, or memory use |

## Retained baseline

All values below come from the retained 2026-07-21 reports on an x86-64 Linux KVM guest with Rust 1.97.1.

| Measurement | Retained result | Important context |
| --- | ---: | --- |
| SigV4 header signing | 10.245 µs median | CPU-only microbenchmark |
| Client construction | 1.3798 µs median | No network or TLS handshake |
| 1 MiB body preparation and integrity verification | 102.07–103.53 MiB/s | In-memory microbenchmark |
| 256 MiB disk-backed managed multipart | 97.14 MiB/s | One loopback run against pinned MinIO |
| Sampled peak-RSS delta for that multipart run | 102.59 MiB | Under the harness's 128 MiB regression envelope |
| Minimal release dependency packages | 91 | Selected comparison features only |
| Stripped minimal binary | 1,402,752 bytes | Local source was dirty during the comparison |

Do not read these numbers as service-level guarantees. In particular, the MinIO transfer was measured once over loopback and the size comparison did not normalize feature completeness across clients.

## Reproduce

Run the Criterion benchmarks:

```sh
cargo bench --features fuzzing --bench validation
```

Run the transfer and RSS harness using [`tools/perf/README.md`](../tools/perf/README.md). Re-run the release-build comparison with:

```sh
./comparisons/size/measure.sh
```

CI preserves the committed `results.json`, measures from a clean tracked source
tree, and compares the minimal `s3-wire` binary against that baseline. The
default failure envelope is 15% binary growth or more than five additional
normal/build dependency packages. Override `SIZE_MAX_GROWTH_PERCENT` and
`SIZE_MAX_DEPENDENCY_GROWTH` only for an intentional, reviewed baseline update.
Build timings are recorded but do not gate shared-runner CI.

The comparison harness retains a representative HEAD operation and records
separate HTTP/1.1-only and HTTP/2-enabled `s3-wire` binaries.

Comparison-only dependencies remain isolated under `comparisons/` and are not included in the published crate.

## Recording a new baseline

Record enough context to make a future comparison meaningful:

- source revision and whether the tree was dirty;
- Rust version, target, and release profile;
- dependency versions and enabled features;
- exact command and object sizes;
- host CPU, memory, operating system, and relevant load;
- object-store version, image digest, and network topology; and
- raw machine-readable output beside the human-readable summary.

Only compare runs when those inputs are sufficiently aligned.

## Interpreting resource use

File-backed PutObject and managed multipart create disk snapshots, so memory does not grow with the complete source size. They do require enough protected temporary storage for the source.

Multipart buffering is bounded by the part size times concurrency reported by
`MultipartOptions::maximum_buffered_bytes()`. HTTP buffers, runtime state,
allocator behavior, and application-owned values add overhead beyond that
derived bound. In-memory multipart also retains the caller's complete
`bytes::Bytes` value.

Streaming GET applies backpressure and does not aggregate the response inside the client. Callers can still grow memory without bound if they retain every yielded chunk.
