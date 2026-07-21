#!/usr/bin/env bash
# shellcheck disable=SC2016 # Markdown backticks are intentional literal text.
set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repository_dir=$(CDPATH='' cd -- "$script_dir/../.." && pwd)
measurement_dir="$script_dir/.measurements"
target_root="$script_dir/targets"
result_file="$script_dir/results.json"
report_file="$script_dir/REPORT.md"
target_triple=$(rustc -vV | sed -n 's/^host: //p')
jobs=${COMPARISON_JOBS:-1}

export CARGO_INCREMENTAL=1
export CARGO_TERM_COLOR=never
export LC_ALL=C
export RUSTFLAGS=""

mkdir -p "$measurement_dir" "$target_root"

case "$jobs" in
    ''|*[!0-9]*|0)
        echo "COMPARISON_JOBS must be a positive integer" >&2
        exit 2
        ;;
esac

measure_project() {
    project=$1
    binary=$2
    manifest="$script_dir/$project/Cargo.toml"
    project_target="$target_root/$project"
    clean_time="$measurement_dir/$project-clean.txt"
    incremental_time="$measurement_dir/$project-incremental.txt"

    cargo clean --manifest-path "$manifest" --target-dir "$project_target"
    /usr/bin/time -f '%e' -o "$clean_time" \
        cargo build --release --locked --jobs "$jobs" \
        --manifest-path "$manifest" --target-dir "$project_target"

    touch "$script_dir/$project/src/main.rs"
    /usr/bin/time -f '%e' -o "$incremental_time" \
        cargo build --release --locked --jobs "$jobs" \
        --manifest-path "$manifest" --target-dir "$project_target"

    dependency_count=$(cargo tree --locked --target "$target_triple" \
        --edges normal,build --prefix none --format '{p}' \
        --manifest-path "$manifest" | sed 's/ (\*)$//' | sort -u \
        | awk 'END { print NR - 1 }')
    binary_path="$project_target/release/$binary"

    jq -n \
        --arg name "$project" \
        --arg version "$(project_version "$project")" \
        --argjson features "$(project_features "$project")" \
        --argjson dependencies "$dependency_count" \
        --argjson clean_seconds "$(<"$clean_time")" \
        --argjson incremental_seconds "$(<"$incremental_time")" \
        --argjson binary_bytes "$(stat -c '%s' "$binary_path")" \
        --arg binary_sha256 "$(sha256sum "$binary_path" | awk '{print $1}')" \
        '{name: $name, version: $version, features: $features,
          dependency_packages: $dependencies,
          clean_build_seconds: $clean_seconds,
          incremental_rebuild_seconds: $incremental_seconds,
          binary_bytes: $binary_bytes, binary_sha256: $binary_sha256}'
}

project_version() {
    case "$1" in
        s3-wire) printf '%s\n' '0.1.0 (local path)' ;;
        aws-sdk-s3) printf '%s\n' '1.138.1' ;;
        rust-s3) printf '%s\n' '0.37.2' ;;
    esac
}

project_features() {
    case "$1" in
        s3-wire) jq -cn '["default-features=false"]' ;;
        aws-sdk-s3) jq -cn '["default-features=false","behavior-version-latest","default-https-client","http-1x","rt-tokio","rustls"]' ;;
        rust-s3) jq -cn '["default-features=false","tokio-rustls-tls"]' ;;
    esac
}

measured_at=$(date -u +'%Y-%m-%dT%H:%M:%SZ')
rustc_version=$(rustc --version)
cargo_version=$(cargo --version)
kernel=$(uname -srmo)
os=$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | tr -d '"')
cpu=$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -1)
git_revision=$(git -C "$repository_dir" rev-parse HEAD)
if test -n "$(git -C "$repository_dir" status --porcelain --untracked-files=all -- Cargo.toml src)"; then
    source_dirty=true
else
    source_dirty=false
fi
source_sha256=$(
    cd "$repository_dir"
    find Cargo.toml src -type f -print0 | sort -z | xargs -0 sha256sum \
        | sha256sum | awk '{print $1}'
)

measure_project s3-wire compare-s3-wire > "$measurement_dir/s3-wire.json"
measure_project aws-sdk-s3 compare-aws-sdk-s3 > "$measurement_dir/aws-sdk-s3.json"
measure_project rust-s3 compare-rust-s3 > "$measurement_dir/rust-s3.json"
projects=$(jq -s '.' \
    "$measurement_dir/s3-wire.json" \
    "$measurement_dir/aws-sdk-s3.json" \
    "$measurement_dir/rust-s3.json")

jq -n \
    --arg measured_at "$measured_at" \
    --arg rustc "$rustc_version" \
    --arg cargo "$cargo_version" \
    --arg target "$target_triple" \
    --arg os "$os" \
    --arg kernel "$kernel" \
    --arg cpu "$cpu" \
    --arg git_revision "$git_revision" \
    --arg source_sha256 "$source_sha256" \
    --argjson source_dirty "$source_dirty" \
    --argjson jobs "$jobs" \
    --argjson projects "$projects" \
    '{schema_version: 1, measured_at_utc: $measured_at,
      environment: {rustc: $rustc, cargo: $cargo, target: $target,
        os: $os, kernel: $kernel, cpu: $cpu, cargo_jobs: $jobs},
      s3_wire_source: {git_revision: $git_revision, dirty: $source_dirty,
        source_sha256: $source_sha256},
      release_profile: {opt_level: 3, lto: "thin", codegen_units: 1,
        panic: "abort", strip: "symbols", incremental: true},
      methodology: {clean_build: "cargo clean followed by cargo build --release --locked",
        incremental_rebuild: "touch src/main.rs followed by cargo build --release --locked",
        dependency_count: "unique normal and build package specifications from cargo tree for the host target, excluding the comparison package"},
      projects: $projects}' > "$result_file"

{
    printf '# S3 client release-build comparison\n\n'
    printf 'Measured `%s` on `%s` (`%s`, `%s`) using `%s`, `%s`, and `%s` with `%s` Cargo job.\n\n' \
        "$measured_at" "$os" "$kernel" "$cpu" "$rustc_version" "$cargo_version" "$target_triple" "$jobs"
    printf 'Local `s3-wire` source: Git revision `%s`, dirty `%s`, source SHA-256 `%s`.\n\n' \
        "$git_revision" "$source_dirty" "$source_sha256"
    printf '| Client | Version | Explicit features | Dependency packages | Clean build | Incremental rebuild | Stripped binary |\n'
    printf '|---|---:|---|---:|---:|---:|---:|\n'
    jq -r '.projects[] | "| \(.name) | \(.version) | `\(.features | join(", "))` | \(.dependency_packages) | \(.clean_build_seconds) s | \(.incremental_rebuild_seconds) s | \(.binary_bytes) bytes |"' "$result_file"
    printf '\n## Method\n\n'
    printf 'Each standalone binary constructs one client for the same region and bucket without making a request. '
    printf 'The two external versions were the newest crates.io releases when resolved on the measurement date and are exact-version pinned. '
    printf 'Every manifest disables default features and lists available TLS/runtime features explicitly; `s3-wire` does not currently feature-gate its runtime or TLS backend. '
    printf 'All three use `opt-level=3`, thin LTO, one codegen unit, `panic="abort"`, symbol stripping, and incremental compilation. '
    printf "The clean timing removes only that project's target directory; registry sources and the Cargo download cache remain warm. "
    printf 'The incremental timing touches only the comparison binary source before rebuilding. Dependency counts include unique normal and build package specifications selected for the host target, not dev dependencies.\n\n'
    printf '## Caveats\n\n'
    printf 'This measures three minimal construction programs, not API completeness, runtime throughput, memory use, or operational correctness. '
    printf 'Feature sets are aligned around Tokio and Rustls where each crate permits it, but crate architectures and feature boundaries differ. '
    printf 'Build timings depend on this machine, filesystem, process load, and warm registry/download caches. '
    printf 'The local `s3-wire` path dependency represents the checked-out source, while external dependencies are exact-version pinned and every comparison has a committed lockfile. '
    printf 'Binary hashes and complete raw values are retained in `results.json`.\n'
} > "$report_file"

printf 'Wrote %s and %s\n' "$result_file" "$report_file"
