# S3 client release-build comparison

Measured `2026-07-21T03:16:39Z` on `Ubuntu 26.04 LTS` (`Linux 7.0.0-1007-ibm x86_64 GNU/Linux`, `Intel Xeon Processor (Cascadelake)`) using `rustc 1.97.1 (8bab26f4f 2026-07-14)`, `cargo 1.97.1 (c980f4866 2026-06-30)`, and `x86_64-unknown-linux-gnu` with `1` Cargo job.

Local `s3-wire` source: Git revision `8d9e412cbf675777fa6927ebab1812224a92a80d`, dirty `true`, source SHA-256 `4c1eff539bf69fda7e9cf0544b6e914c40b1b00ec539bd3f28944004e53132d0`.

| Client | Version | Explicit features | Dependency packages | Clean build | Incremental rebuild | Stripped binary |
|---|---:|---|---:|---:|---:|---:|
| s3-wire | 0.1.0 (local path) | `default-features=false` | 91 | 93.82 s | 10.19 s | 1402752 bytes |
| aws-sdk-s3 | 1.138.1 | `default-features=false, behavior-version-latest, default-https-client, http-1x, rt-tokio, rustls` | 170 | 360.78 s | 55.02 s | 9235720 bytes |
| rust-s3 | 0.37.2 | `default-features=false, tokio-rustls-tls` | 136 | 230.71 s | 22.92 s | 3133880 bytes |

## Method

Each standalone binary constructs one client for the same region and bucket without making a request. The two external versions were the newest crates.io releases when resolved on the measurement date and are exact-version pinned. Every manifest disables default features and lists available TLS/runtime features explicitly; `s3-wire` does not currently feature-gate its runtime or TLS backend. All three use `opt-level=3`, thin LTO, one codegen unit, `panic="abort"`, symbol stripping, and incremental compilation. The clean timing removes only that project's target directory; registry sources and the Cargo download cache remain warm. The incremental timing touches only the comparison binary source before rebuilding. Dependency counts include unique normal and build package specifications selected for the host target, not dev dependencies.

## Caveats

This measures three minimal construction programs, not API completeness, runtime throughput, memory use, or operational correctness. Feature sets are aligned around Tokio and Rustls where each crate permits it, but crate architectures and feature boundaries differ. Build timings depend on this machine, filesystem, process load, and warm registry/download caches. The local `s3-wire` path dependency represents the checked-out source, while external dependencies are exact-version pinned and every comparison has a committed lockfile. Binary hashes and complete raw values are retained in `results.json`.

The harness now retains a representative HEAD-object operation in each binary.
The next recorded baseline will supersede these construction-only numbers rather
than being directly comparable to them.
