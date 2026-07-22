## Summary

Describe the externally visible behavior and why the change is needed.

## Validation

List commands, provider suites, fuzz targets, or benchmarks run. Explain any checks that were not run.

## Bounds and security

- [ ] New memory, disk, concurrency, timeout, and response-size bounds are explicit and tested, or are not applicable.
- [ ] Retry behavior proves request-body replayability and uncertain outcomes are handled, or are not applicable.
- [ ] Credential-bearing values, signed URLs, upload IDs, and remote diagnostics remain redacted, or are not applicable.
- [ ] No credentials, private bucket names, signed URL queries, or object data appear in the change or its logs.

## Compatibility and release notes

- [ ] Public API and provider-compatibility effects are documented.
- [ ] User-visible changes are recorded under `CHANGELOG.md` `[Unreleased]`.
- [ ] Breaking changes include migration guidance and the versioning impact.
