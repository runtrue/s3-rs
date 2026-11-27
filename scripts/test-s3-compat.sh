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
minio_dir=
minio_pid=

cleanup() {
    if [ "$provider" = minio ]; then
        if [ -n "$minio_pid" ]; then
            kill "$minio_pid" 2>/dev/null || true
            wait "$minio_pid" 2>/dev/null || true
        fi
        if [ -n "$minio_dir" ]; then
            rm -rf -- "$minio_dir"
        fi
    else
        docker rm --force "$container" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

case "$provider" in
    minio)
        release='RELEASE.2025-09-07T16-13-09Z'
        case "$(uname -s):$(uname -m)" in
            Linux:x86_64)
                platform=linux-amd64
                checksum=7c5bd8512c6e966455b1d198209358b2d191c77a83ab377c4073281065fb855f
                ;;
            Linux:aarch64|Linux:arm64)
                platform=linux-arm64
                checksum=5c83cd2cf151717ba0243f73e1c7802ff36e272b67144bdd7f1f7d684fd6f03d
                ;;
            Darwin:x86_64)
                platform=darwin-amd64
                checksum=4759080aeef7385aaceaac1131c30aaeb99605921553967dc6d3ef4e16ac64f9
                ;;
            Darwin:arm64)
                platform=darwin-arm64
                checksum=7c3b3039b76e55a1b80935848ed83998d5e8d317374f87851f46a019ff5c0aa4
                ;;
            *)
                printf 'the MinIO runner supports Linux and macOS on x86-64 and ARM64\n' >&2
                exit 2
                ;;
        esac
        asset="minio.$platform.$release"
        minio_dir=$(mktemp -d "${TMPDIR:-/tmp}/s3-wire-minio.XXXXXXXX")
        curl --fail --silent --show-error --location --retry 3 --max-time 120 \
            "https://github.com/minio/minio/releases/download/$release/$asset" \
            --output "$minio_dir/minio"
        (cd "$minio_dir" && printf '%s  minio\n' "$checksum" | shasum -a 256 -c -)
        chmod +x "$minio_dir/minio"
        mkdir -p "$minio_dir/data/$S3_COMPAT_BUCKET"
        MINIO_ROOT_USER="$S3_COMPAT_ACCESS_KEY" MINIO_ROOT_PASSWORD="$S3_COMPAT_SECRET_KEY" \
            MINIO_BROWSER=off MINIO_UPDATE=off \
            "$minio_dir/minio" server "$minio_dir/data" --address 127.0.0.1:0 \
            > "$minio_dir/server.log" 2>&1 &
        minio_pid=$!
        fixture="$asset"
        ready_path='/minio/health/ready'
        ;;
    rustfs)
        # RustFS 1.0.0-beta.9, pinned to a manifest-list digest.
        image='rustfs/rustfs@sha256:f75d0bca6ca322c4e59f7125f73dd9ab709b22f71396a42c95bce7f74c99e53b'
        fixture="$image"
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
        fixture="$image"
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

if [ "$provider" != minio ]; then
    host_port=$(docker port "$container" "$port/tcp" | sed -n '1s/.*://p')
    S3_COMPAT_ENDPOINT="http://127.0.0.1:$host_port"
fi

attempt=0
ready() {
    case "$provider" in
        minio)
            S3_COMPAT_ENDPOINT=$(awk '$1 == "API:" && $2 ~ /^http:\/\/127\.0\.0\.1:[0-9]+$/ { print $2; exit }' "$minio_dir/server.log")
            [ -n "$S3_COMPAT_ENDPOINT" ] && curl --fail --silent --output /dev/null "$S3_COMPAT_ENDPOINT$ready_path"
            ;;
        seaweedfs)
            docker logs "$container" 2>&1 | grep -Fq "created bucket $S3_COMPAT_BUCKET"
            ;;
        *)
            curl --fail --silent --output /dev/null "$S3_COMPAT_ENDPOINT$ready_path"
            ;;
    esac
}

until ready; do
    attempt=$((attempt + 1))
    if [ "$provider" = minio ]; then
        if [ "$attempt" -ge 300 ] || ! kill -0 "$minio_pid" 2>/dev/null; then
            cat "$minio_dir/server.log" >&2
            exit 1
        fi
    elif [ "$attempt" -ge 300 ]; then
        docker logs "$container"
        exit 1
    fi
    sleep 0.1
done

export S3_COMPAT_PROVIDER="$provider"
export S3_COMPAT_ENDPOINT S3_COMPAT_BUCKET S3_COMPAT_ACCESS_KEY S3_COMPAT_SECRET_KEY

printf '%s (%s) at %s\n' "$provider" "$fixture" "$S3_COMPAT_ENDPOINT"
cargo test --test s3_compat -- --ignored --nocapture
