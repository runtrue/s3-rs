#!/bin/sh
set -eu

provider=${1:-}
if [ -z "$provider" ]; then
    printf 'usage: %s <minio|rustfs|seaweedfs>\n' "$0" >&2
    exit 2
fi

S3_COMPAT_BUCKET='s3-wire-tests'
S3_COMPAT_ACCESS_KEY='s3wiretest'
S3_COMPAT_SECRET_KEY='s3wiretest-secret-key'
container="s3-wire-$provider-$$"

cleanup() {
    docker rm --force "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

case "$provider" in
    minio)
        # MinIO RELEASE.2025-09-07T16-13-09Z, pinned to a manifest-list digest.
        image='minio/minio@sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e'
        port=9000
        ready_path='/minio/health/ready'
        docker run --detach --rm \
            --name "$container" \
            --publish "127.0.0.1::$port" \
            --env MINIO_ROOT_USER="$S3_COMPAT_ACCESS_KEY" \
            --env MINIO_ROOT_PASSWORD="$S3_COMPAT_SECRET_KEY" \
            --env MINIO_TEST_BUCKET="$S3_COMPAT_BUCKET" \
            --entrypoint /bin/sh \
            "$image" \
            -c 'mkdir -p "/data/$MINIO_TEST_BUCKET" && exec minio server /data --address :9000' \
            >/dev/null
        ;;
    rustfs)
        # RustFS 1.0.0-beta.9, pinned to a manifest-list digest.
        image='rustfs/rustfs@sha256:f75d0bca6ca322c4e59f7125f73dd9ab709b22f71396a42c95bce7f74c99e53b'
        port=9000
        ready_path='/health/ready'
        docker run --detach --rm \
            --name "$container" \
            --publish "127.0.0.1::$port" \
            --env RUSTFS_ACCESS_KEY="$S3_COMPAT_ACCESS_KEY" \
            --env RUSTFS_SECRET_KEY="$S3_COMPAT_SECRET_KEY" \
            --env S3_COMPAT_BUCKET="$S3_COMPAT_BUCKET" \
            --entrypoint /bin/sh \
            "$image" \
            -c 'mkdir -p "/data/$S3_COMPAT_BUCKET" && exec /entrypoint.sh rustfs' \
            >/dev/null
        ;;
    seaweedfs)
        # SeaweedFS 4.40, pinned to a manifest-list digest.
        image='chrislusf/seaweedfs@sha256:52194fba4fecd0083c842158b3a902ba6e04a63619b2b0efcd08007bdb6a4602'
        port=8333
        ready_path='/'
        docker run --detach --rm \
            --name "$container" \
            --publish "127.0.0.1::$port" \
            --env AWS_ACCESS_KEY_ID="$S3_COMPAT_ACCESS_KEY" \
            --env AWS_SECRET_ACCESS_KEY="$S3_COMPAT_SECRET_KEY" \
            --env S3_BUCKET="$S3_COMPAT_BUCKET" \
            "$image" \
            >/dev/null
        ;;
    *)
        printf 'unsupported S3 provider: %s\n' "$provider" >&2
        exit 2
        ;;
esac

host_port=$(docker port "$container" "$port/tcp" | sed -n '1s/.*://p')
S3_COMPAT_ENDPOINT="http://127.0.0.1:$host_port"

attempt=0
ready() {
    if [ "$provider" = seaweedfs ]; then
        docker logs "$container" 2>&1 | grep -Fq "created bucket $S3_COMPAT_BUCKET"
    else
        curl --fail --silent --output /dev/null "$S3_COMPAT_ENDPOINT$ready_path"
    fi
}

until ready; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 300 ]; then
        docker logs "$container"
        exit 1
    fi
    sleep 0.1
done

export S3_COMPAT_PROVIDER="$provider"
export S3_COMPAT_ENDPOINT S3_COMPAT_BUCKET S3_COMPAT_ACCESS_KEY S3_COMPAT_SECRET_KEY

printf '%s (%s) at %s\n' "$provider" "$image" "$S3_COMPAT_ENDPOINT"
cargo test --test s3_compat -- --ignored --nocapture
