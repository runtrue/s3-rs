# Microbenchmark baseline — 2026-07-21

This baseline was collected from commit `8d9e412` plus the validation changes on an
x86-64 Linux KVM guest with 16 vCPUs reported as Intel Xeon Cascade Lake and 32 GiB RAM.
The compiler was `rustc 1.97.1 (8bab26f4f 2026-07-14)` with the repository's release
profile. These are short local samples, not cross-machine performance claims.

Command:

```sh
cargo bench --features fuzzing --bench validation -- \
  --warm-up-time 0.2 --measurement-time 0.5 --sample-size 10
```

| Benchmark | Median estimate | 95% interval / throughput interval |
| --- | ---: | ---: |
| Canonical URI | 154.09 ns | 150.66–161.28 ns |
| Canonical query, 64 pairs | 9.3294 µs | 9.2342–9.3819 µs |
| Canonical headers, 33 headers | 9.6005 µs | 9.4817–9.7014 µs |
| SigV4 header signing | 10.245 µs | 10.157–10.383 µs |
| S3 error XML parse | 2.3061 µs | 2.2886–2.3285 µs |
| ListObjectsV2 XML parse | 4.1907 µs | 4.1720–4.2088 µs |
| Multipart XML parse set | 5.8438 µs | 5.7369–5.9499 µs |
| Client construction | 1.3798 µs | 1.3703–1.3893 µs |
| 4 KiB in-memory body | 37.764 µs | 102.73–104.26 MiB/s |
| 64 KiB in-memory body | 586.60 µs | 106.20–106.76 MiB/s |
| 1 MiB in-memory body | 9.7331 ms | 102.07–103.53 MiB/s |
| Multipart concurrency calculation | 3.4970 ns | 3.4845–3.5144 ns |

The in-memory body benchmark includes SHA-256 preparation and body integrity verification;
it does not include HTTP, TLS, disk, or object-store latency. Networked MinIO measurements
are retained separately.
