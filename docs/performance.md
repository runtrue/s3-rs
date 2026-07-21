# Performance and size

## Measurement policy

Performance data in this repository describes a specific command, revision, toolchain, machine, dependency set, and service configuration. Results are retained with caveats and are not claims about other machines, networks, object stores, or workloads.

The repository retains three complementary sets of data:

- [Microbenchmark baseline](../measurements/microbench-2026-07-21.md) for canonicalization, signing, XML parsing, client construction, in-memory body preparation, and multipart scheduling.
- [Local MinIO transfer measurement](../measurements/results/minio-local.md) for a single PUT, streaming GET, managed multipart, disk-backed multipart, and sampled process RSS.
- [Release-build comparison](../comparisons/size/REPORT.md) for dependency package count, clean build, incremental rebuild, and stripped minimal binary size.

## Retained baseline

The 2026-07-21 microbenchmark run used Rust 1.97.1 on an x86-64 Linux KVM guest. Its median estimates included 10.245 µs for SigV4 header signing, 1.3798 µs for client construction, and 102.07–103.53 MiB/s for 1 MiB in-memory body preparation and integrity verification. These operations did not include network, TLS, disk, or object-store latency.

The local MinIO run used the pinned `RELEASE.2025-09-07T16-13-09Z` image on the same class of host. It recorded a 256 MiB disk-backed managed multipart upload at 97.14 MiB/s and a 102.59 MiB sampled peak-RSS delta, under that harness's 128 MiB regression envelope. Transfers were measured once over loopback; the result is useful as a retained regression point, not as a capacity estimate.

The release-build comparison used minimal client-construction binaries and exact external client versions. For the selected features it recorded 91 dependency packages and a 1,402,752-byte stripped binary for the local `s3-wire` build. The report does not measure API completeness, runtime correctness, throughput, or memory use, and the local source was dirty at measurement time.

## Reproducing measurements

Run the Criterion benchmark:

```sh
cargo bench --features fuzzing --bench validation
```

Run the MinIO performance harness according to `tools/perf/README.md`. Re-run the build comparison with `comparisons/size/measure.sh`; it keeps comparison-only dependencies outside the published crate.

Do not compare new numbers with the retained baseline unless the command, release profile, object sizes, service image, dependency features, and host conditions are sufficiently aligned. Record raw JSON alongside a human-readable report and state whether the source tree was dirty.

## Resource interpretation

File-backed PutObject and managed multipart use disk snapshots so memory does not scale with the complete source size. Multipart memory is governed by part size, concurrency, and the total in-flight byte budget, but HTTP buffers, runtime state, allocator behavior, and application-owned bytes add overhead. In-memory managed multipart necessarily retains the caller's complete `bytes::Bytes` value.

Streaming GET applies backpressure and does not aggregate the response, but caller behavior determines whether chunks are retained. The response stream performs length and supported checksum validation as bytes pass through it.
