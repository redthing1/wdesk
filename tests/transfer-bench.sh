#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
image="${WDESK_TEST_IMAGE:-windows-lite}"
engine="${1:-native}"
mib="${WDESK_TEST_TRANSFER_MIB:-61}"
deadline="${WDESK_TEST_TRANSFER_TIMEOUT:-300}"
for tool in jq timeout date head cmp; do
    command -v "$tool" >/dev/null || { echo "Required tool missing: $tool" >&2; exit 1; }
done
[[ "$mib" =~ ^[1-9][0-9]*$ ]] && [ "$mib" -le 4096 ]
[[ "$deadline" =~ ^[1-9][0-9]*$ ]] && [ "$deadline" -le 3600 ]
case "$engine" in native|docker|podman) ;; *) echo 'Expected native, docker, or podman' >&2; exit 1 ;; esac
# Never let an inherited agent descriptor redirect this owner's scratch test.
unset WDESK_DESCRIPTOR
session="transfer-bench-$engine-$$"
scratch="$(mktemp -d /tmp/wdesk-transfer-bench.XXXXXXXX)"
bytes=$((mib * 1024 * 1024))
w() { "$bin" --session "$session" --json "$@"; }
cleanup() { w stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "Transfer benchmark artifacts: $scratch" >&2
w open --image "$image" --engine "$engine" --offline \
    --memory "${WDESK_TEST_MEMORY:-2048}" --cpus "${WDESK_TEST_CPUS:-2}" \
    --timeout 240 >"$scratch/open.json"
ready_deadline=$((SECONDS + 60))
until w capabilities >"$scratch/capabilities.json" && \
    jq -e '.guest.transport == "tcp_guestfwd"' "$scratch/capabilities.json" >/dev/null; do
    [ "$SECONDS" -lt "$ready_deadline" ] || { echo 'Fast helper transport unavailable' >&2; exit 1; }
    sleep 2
done
head -c "$bytes" /dev/urandom >"$scratch/input.bin"
measure() {
    local operation="$1" started finished code
    shift
    started="$(date +%s%3N)"
    if timeout --signal=INT --kill-after=5s "$deadline" \
        "$bin" --session "$session" --json "$@" \
        >"$scratch/$operation.json" 2>"$scratch/$operation.stderr"; then
        code=0
    else
        code=$?
    fi
    finished="$(date +%s%3N)"
    jq -n --arg operation "$operation" --arg engine "$engine" \
        --argjson bytes "$bytes" --argjson started "$started" \
        --argjson finished "$finished" --argjson exit_code "$code" \
        '{operation:$operation,engine:$engine,bytes:$bytes,
          elapsed_seconds:(($finished-$started)/1000),exit_code:$exit_code,
          complete:($exit_code==0),mib_per_second:
            (if $exit_code==0 and $finished>$started
             then ($bytes/1048576)/(($finished-$started)/1000) else null end)}' \
        | tee "$scratch/$operation-measurement.json"
    if [ "$code" -ne 0 ]; then
        echo "$operation failed (exit $code); see $scratch/$operation.stderr" >&2
        return "$code"
    fi
}
measure import import "$scratch/input.bin" workspace/transfer-bench.bin
measure export export workspace/transfer-bench.bin "$scratch/output.bin"
cmp "$scratch/input.bin" "$scratch/output.bin"
w delete >/dev/null
trap - EXIT
echo "Transfer integrity verified; artifacts retained in $scratch" >&2
