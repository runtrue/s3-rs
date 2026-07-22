# Local MinIO measurement

This opt-in tool records a small, reproducible local workload against MinIO. It
is a measurement harness, not a cross-client benchmark. The tool is a separate,
non-publishable Cargo package and is excluded from the `s3-wire` crate package.

Run the default 64 MiB workload with the MinIO image pinned by digest:

```sh
./tools/perf/run-minio.sh
```

The runner writes `measurements/results/minio-local.json` and
`measurements/results/minio-local.md`. It measures the first `S3Client::new`
call, one warmed single-request PUT, one streaming GET consumed without retaining
the body, one managed multipart upload from memory, and one 256 MiB disk-backed
managed multipart upload. The disk source is generated with a reusable 64 KiB
buffer, and its reported timing includes the client's immutable disk snapshot.
Transfer timings include client-side hashing and signing.

The workload can be adjusted through `S3_PERF_PAYLOAD_MIB`,
`S3_PERF_LARGE_FILE_MIB`,
`S3_PERF_PART_MIB`, and `S3_PERF_MULTIPART_CONCURRENCY`. Output paths can be set
with `S3_PERF_JSON` and `S3_PERF_MARKDOWN`.

If `MINIO_S3_ENDPOINT` is already set, the runner uses that endpoint instead of
starting a container. In that mode, also provide `MINIO_S3_BUCKET`,
`MINIO_ROOT_USER`, and `MINIO_ROOT_PASSWORD`; the bucket must already exist.

RSS values come from Linux `/proc`, sampled once per millisecond. The report
includes absolute peaks, peak deltas from the start of each phase, and the delta
as a fraction of object size. These are process-wide observations and should not
be interpreted as per-operation allocation counts. The aggregate peak is
maintained by the same sampler; kernel `VmHWM` is omitted because its accounting
can lag sampled `VmRSS`.

The runner also fails if the disk-backed phase's sampled RSS delta exceeds four
times the multipart options' derived in-flight byte bound. This conservative
regression envelope accounts for transport copies, runtime state, and allocator
behavior; it is not a precise allocation model or a proof of constant memory.
