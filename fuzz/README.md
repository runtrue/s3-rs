# Fuzzing

Install `cargo-fuzz`, then run an individual target from the repository root:

```text
cargo fuzz run canonical_uri
cargo fuzz run canonical_query
cargo fuzz run header_canonicalization
cargo fuzz run s3_error_xml
cargo fuzz run list_objects_v2_xml
cargo fuzz run multipart_xml
cargo fuzz run endpoint_construction
```

Keep useful minimized inputs under `fuzz/corpus/<target>/`. Crash artifacts are
ignored and must be reduced before they become regression tests.
