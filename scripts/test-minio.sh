#!/bin/sh
set -eu

# Backward-compatible entry point for local users and existing automation.
exec "$(dirname "$0")/test-s3-compat.sh" minio
