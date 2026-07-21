#!/bin/sh
set -eu

TOOL_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_DIR=$(CDPATH='' cd -- "$TOOL_DIR/../.." && pwd)

# Keep this identical to scripts/test-minio.sh. It is an official MinIO image
# pinned by manifest-list digest rather than a mutable tag.
MINIO_IMAGE=${MINIO_IMAGE:-'minio/minio@sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e'}
MINIO_RELEASE=${MINIO_RELEASE:-'RELEASE.2025-09-07T16-13-09Z'}
MINIO_S3_BUCKET=${MINIO_S3_BUCKET:-'s3-wire-perf'}
MINIO_ROOT_USER=${MINIO_ROOT_USER:-'s3wireperf'}
MINIO_ROOT_PASSWORD=${MINIO_ROOT_PASSWORD:-'s3wireperf-secret-key'}
S3_PERF_BUILD_PROFILE=${S3_PERF_BUILD_PROFILE:-'release'}
STARTED_CONTAINER=''

cleanup() {
    if [ -n "$STARTED_CONTAINER" ]; then
        docker rm --force "$STARTED_CONTAINER" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT INT TERM

if [ -z "${MINIO_S3_ENDPOINT:-}" ]; then
    STARTED_CONTAINER="s3-wire-perf-$$"
    docker run --detach --rm \
        --name "$STARTED_CONTAINER" \
        --publish 127.0.0.1::9000 \
        --env MINIO_ROOT_USER="$MINIO_ROOT_USER" \
        --env MINIO_ROOT_PASSWORD="$MINIO_ROOT_PASSWORD" \
        --env MINIO_PERF_BUCKET="$MINIO_S3_BUCKET" \
        --entrypoint /bin/sh \
        "$MINIO_IMAGE" \
        -c 'mkdir -p "/data/$MINIO_PERF_BUCKET" && exec minio server /data --address :9000' \
        >/dev/null

    MINIO_PORT=$(docker port "$STARTED_CONTAINER" 9000/tcp | sed -n '1s/.*://p')
    MINIO_S3_ENDPOINT="http://127.0.0.1:$MINIO_PORT"

    attempt=0
    until curl --fail --silent "$MINIO_S3_ENDPOINT/minio/health/ready" >/dev/null; do
        attempt=$((attempt + 1))
        if [ "$attempt" -ge 100 ]; then
            docker logs "$STARTED_CONTAINER"
            exit 1
        fi
        sleep 0.1
    done
fi

export MINIO_IMAGE MINIO_RELEASE MINIO_S3_ENDPOINT MINIO_S3_BUCKET
export MINIO_ROOT_USER MINIO_ROOT_PASSWORD S3_PERF_BUILD_PROFILE

cd "$REPOSITORY_DIR"
printf 'Measuring against MinIO %s (%s) at %s\n' "$MINIO_RELEASE" "$MINIO_IMAGE" "$MINIO_S3_ENDPOINT"
cargo run --release --locked --manifest-path "$TOOL_DIR/Cargo.toml"
