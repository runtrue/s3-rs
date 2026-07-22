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

The nightly workflow gives every target a three-minute wall-clock budget, a
ten-second per-input timeout, and a 2 GiB RSS ceiling. It starts from the
checked-in seed corpus and uploads the evolved corpus plus any crash artifacts
for 30 days. Artifacts are evidence and triage inputs, not trusted regression
tests: minimize and inspect a reproducer, remove sensitive data, then commit it
to the target corpus and add a deterministic test when practical.
