#!/usr/bin/env bash
set -euo pipefail

if test "$#" -ne 2; then
    echo "usage: $0 BASELINE.json CURRENT.json" >&2
    exit 2
fi

baseline=$1
current=$2
maximum_growth_percent=${SIZE_MAX_GROWTH_PERCENT:-15}
maximum_dependency_growth=${SIZE_MAX_DEPENDENCY_GROWTH:-5}

case "$maximum_growth_percent:$maximum_dependency_growth" in
    *[!0-9:]*|:*|*:)
        echo "size regression limits must be non-negative integers" >&2
        exit 2
        ;;
esac

jq -e '.schema_version == 1 and (.projects | type == "array")' "$baseline" >/dev/null
jq -e '.schema_version == 1 and (.projects | type == "array")' "$current" >/dev/null

baseline_bytes=$(jq -er '.projects[] | select(.name == "s3-wire") | .binary_bytes' "$baseline")
current_bytes=$(jq -er '.projects[] | select(.name == "s3-wire") | .binary_bytes' "$current")
baseline_dependencies=$(jq -er '.projects[] | select(.name == "s3-wire") | .dependency_packages' "$baseline")
current_dependencies=$(jq -er '.projects[] | select(.name == "s3-wire") | .dependency_packages' "$current")

maximum_bytes=$((baseline_bytes + (baseline_bytes * maximum_growth_percent / 100)))
maximum_dependencies=$((baseline_dependencies + maximum_dependency_growth))

printf '# Release-size regression\n\n'
printf '| Metric | Baseline | Current | Allowed |\n'
printf '|---|---:|---:|---:|\n'
printf '| Stripped binary bytes | %s | %s | %s |\n' \
    "$baseline_bytes" "$current_bytes" "$maximum_bytes"
printf '| Dependency packages | %s | %s | %s |\n' \
    "$baseline_dependencies" "$current_dependencies" "$maximum_dependencies"

status=0
if test "$current_bytes" -gt "$maximum_bytes"; then
    echo "binary size exceeded the ${maximum_growth_percent}% growth envelope" >&2
    status=1
fi
if test "$current_dependencies" -gt "$maximum_dependencies"; then
    echo "dependency count exceeded the +${maximum_dependency_growth} package envelope" >&2
    status=1
fi
exit "$status"
