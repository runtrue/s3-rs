#!/bin/sh
set -eu

# Official MinIO image. The manifest-list digest was resolved from Docker Hub
# with `docker pull` and checked with `docker manifest inspect`.
MINIO_IMAGE='minio/minio@sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e'
MINIO_RELEASE='RELEASE.2025-09-07T16-13-09Z'
MINIO_CONTAINER="s3-wire-minio-$$"
MINIO_S3_BUCKET='s3-wire-tests'
MINIO_ROOT_USER='s3wiretest'
MINIO_ROOT_PASSWORD='s3wiretest-secret-key'

cleanup() {
    docker rm --force "$MINIO_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

docker run --detach --rm \
    --name "$MINIO_CONTAINER" \
    --publish 127.0.0.1::9000 \
    --env MINIO_ROOT_USER="$MINIO_ROOT_USER" \
    --env MINIO_ROOT_PASSWORD="$MINIO_ROOT_PASSWORD" \
    --env MINIO_TEST_BUCKET="$MINIO_S3_BUCKET" \
    --entrypoint /bin/sh \
    "$MINIO_IMAGE" \
    -c 'mkdir -p "/data/$MINIO_TEST_BUCKET" && exec minio server /data --address :9000' \
    >/dev/null

MINIO_PORT=$(docker port "$MINIO_CONTAINER" 9000/tcp | sed -n '1s/.*://p')
MINIO_S3_ENDPOINT="http://127.0.0.1:$MINIO_PORT"

attempt=0
until curl --fail --silent "$MINIO_S3_ENDPOINT/minio/health/ready" >/dev/null; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 100 ]; then
        docker logs "$MINIO_CONTAINER"
        exit 1
    fi
    sleep 0.1
done

export MINIO_S3_ENDPOINT MINIO_S3_BUCKET MINIO_ROOT_USER MINIO_ROOT_PASSWORD

printf 'MinIO %s (%s) at %s\n' "$MINIO_RELEASE" "$MINIO_IMAGE" "$MINIO_S3_ENDPOINT"
cargo test --test minio -- --ignored --nocapture
