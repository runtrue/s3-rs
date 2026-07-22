# Security policy

## Supported versions

Security fixes are applied to the default branch and the latest supported release line. The `0.0.0` name-establishment package and older development revisions are not maintained as separate security branches.

## Reporting a vulnerability

Report suspected vulnerabilities through [GitHub's private vulnerability reporting](https://github.com/runtrue/s3-rs/security/advisories/new). Do not open a public issue for a vulnerability that could expose credentials, signed URLs, object contents, or a remotely exploitable parsing or transport flaw.

Include the affected version or revision, deployment conditions, reproduction steps, expected impact, and any known mitigations. Remove real credentials, authorization headers, signed URL query strings, bucket names, and object data from the report. Use synthetic credentials when a reproduction requires request material.

The maintainers will acknowledge the report, validate its scope, coordinate a fix and release when needed, and credit reporters who request attribution. Public disclosure should wait until a fix or mitigation is available to affected users.

## Security boundaries

The client validates TLS by default, signs requests with SigV4, bounds remote XML and error bodies, and redacts credential-bearing values from standard formatting. Application operators remain responsible for protecting process memory, environment variables, temporary storage, network endpoints, IAM policy, bucket policy, and presigned URLs after explicitly exposing them.

See [the security model](docs/security-model.md) for the threat model, controls, and residual risks.
