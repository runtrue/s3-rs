# Local MinIO measurement

- Generated: `2026-07-21T03:25:39.379735926Z`
- Revision: `8d9e412cbf675777fa6927ebab1812224a92a80d` (dirty)
- MinIO: `RELEASE.2025-09-07T16-13-09Z`
- Image: `minio/minio@sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e`
- Host: `Linux 7.0.0-1007-ibm #7-Ubuntu SMP PREEMPT Tue May 26 16:41:08 UTC 2026 x86_64 GNU/Linux`
- CPU: `Intel Xeon Processor (Cascadelake)` (16 logical CPUs)
- Rust: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- In-memory payload: 64 MiB (`every byte is 0xa5`)
- Disk-backed payload: 256 MiB (generated with a 64 KiB buffer)
- Multipart: 8 MiB × concurrency 4

| Measurement | Object size | Elapsed | Throughput | RSS before | Peak RSS | Peak RSS delta | Delta/object |
|---|---:|---:|---:|---:|---:|---:|---:|
| Cold `S3Client` construction | — | 0.000038 s | — | 4.27 MiB | 4.61 MiB | 0.34 MiB | — |
| Disk-backed managed multipart upload | 256.00 MiB | 2.635347 s | 97.14 MiB/s | 5.41 MiB | 108.01 MiB | 102.59 MiB | 40.08% |
| Single PUT | 64.00 MiB | 0.951375 s | 67.27 MiB/s | 172.02 MiB | 172.02 MiB | 0.00 MiB | 0.00% |
| Streaming GET to sink | 64.00 MiB | 0.058674 s | 1090.77 MiB/s | 172.02 MiB | 172.02 MiB | 0.00 MiB | 0.01% |
| Managed multipart upload | 64.00 MiB | 0.501861 s | 127.53 MiB/s | 172.02 MiB | 172.02 MiB | 0.00 MiB | 0.00% |

Disk-backed RSS bound: **PASS** — observed 102.59 MiB, limit 128.00 MiB (four times the configured multipart in-flight byte budget).

## Method and caveats

- This is a local single-process MinIO measurement, not a comparative benchmark.
- Each transfer is measured once; scheduler, filesystem, loopback, CPU scaling, and container state affect results.
- Transfer timing includes client hashing, signing, serialization, and response handling; server startup and the warm-up request are excluded.
- GET is consumed incrementally without retaining the body and verifies byte count, but does not persist or hash the downloaded bytes.
- RSS is sampled from Linux /proc every 1 ms and is process-wide; allocator retention and prior phases make per-phase peaks non-isolated.
- The disk-backed phase generates its configured source with one reusable 64 KiB buffer, then includes the client's disk snapshot and bounded multipart upload in its timing.
- The opt-in regression check permits four times the configured multipart in-flight budget for part buffers, transport copies, runtime state, and allocator behavior; it is an engineering envelope, not a precise allocation model.
- A peak-RSS delta at one object size demonstrates this run's bounded behavior but is not a proof of asymptotic memory usage; Linux page cache is outside process RSS.
- The aggregate process peak uses the same RSS sampler as phase peaks; kernel VmHWM is intentionally omitted because its accounting can lag sampled VmRSS.
- Cold client construction means the first S3Client::new call in this process; configuration construction is excluded.
- The fixed repeated-byte payload is deterministic but does not model every production workload.
