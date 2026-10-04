#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
image="${WDESK_TEST_IMAGE:-windows-lite}"
engine="${1:-native}"
deadline="${WDESK_TEST_RECOVERY_TIMEOUT:-20}"
case "$engine" in native|docker|podman) ;; *) echo 'Expected native, docker, or podman' >&2;exit 1;; esac
[[ "$deadline" =~ ^[1-9][0-9]*$ ]] && [ "$deadline" -le 300 ]
for tool in curl jq timeout date head sed cmp; do command -v "$tool" >/dev/null; done
unset WDESK_DESCRIPTOR
session="transfer-recovery-$engine-$$"
scratch="$(mktemp -d /tmp/wdesk-transfer-recovery.XXXXXXXX)"
cli=''
w() { "$bin" --session "$session" --json "$@"; }
cleanup() {
    if [ -n "$cli" ]; then kill -TERM "$cli" 2>/dev/null || true;wait "$cli" 2>/dev/null || true;fi
    w stop >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "Active transfer recovery artifacts: $scratch" >&2
w open --image "$image" --engine "$engine" --offline \
    --memory "${WDESK_TEST_MEMORY:-2048}" --cpus "${WDESK_TEST_CPUS:-2}" --timeout 240 >"$scratch/open.json"
w capabilities >"$scratch/capabilities.json"
jq -e '.file_transfer.binary_stream and .file_transfer.range_identity' "$scratch/capabilities.json" >/dev/null || {
    echo 'Prepare a current helper: correlated binary ranges are required' >&2;exit 1
}
w connect "$scratch/client.json" >/dev/null
endpoint=$(jq -r .endpoint "$scratch/client.json")
token=$(jq -r .token "$scratch/client.json")
epoch=$(jq -r .epoch "$scratch/client.json")
helper=$(w status | jq -r .guest.helper_id)
identity=$(jq -nc --arg epoch "$epoch" --arg helper "$helper" '{epoch:$epoch,helper_id:$helper}')
api() { curl --silent --show-error --fail-with-body --max-time 10 -H "Authorization: Bearer $token" "$@"; }
head -c 63963136 /dev/urandom >"$scratch/source.bin"
for operation in import export; do
    started=$(date +%s%3N)
    if [ "$operation" = import ]; then
        timeout --signal=INT --kill-after=5s "$deadline" "$bin" --session "$session" --json \
            import "$scratch/source.bin" workspace/recovery.bin >"$scratch/import.json" 2>"$scratch/import.stderr" &
    else
        timeout --signal=INT --kill-after=5s "$deadline" "$bin" --session "$session" --json \
            export workspace/recovery.bin "$scratch/received.bin" >"$scratch/export.json" 2>"$scratch/export.stderr" &
    fi
    cli=$!
    # Pause the guest range while the same CLI invocation is actively streaming.
    # The host's QEMU socket can remain open after this guest TCP disconnect.
    while true; do
        id=$(sed -nE "s/^wdesk $operation ([a-f0-9-]+): starting.*/\1/p" "$scratch/$operation.stderr" | head -n 1)
        if [ -n "$id" ] && api "$endpoint/v1/transfers/$id?epoch=$epoch&helper_id=$helper" >"$scratch/status.json" 2>/dev/null && \
            jq -e '.active and (.range_id!=null)' "$scratch/status.json" >/dev/null; then break;fi
        kill -0 "$cli" 2>/dev/null || { echo "$operation exited before fault injection" >&2;exit 1; }
        sleep 0.02
    done
    api -H 'Content-Type: application/json' -d "$identity" \
        "$endpoint/v1/transfers/$id/pause" >"$scratch/$operation-pause.json"
    wait "$cli"
    cli=''
    finished=$(date +%s%3N)
    jq -e --arg id "$id" '.transfer==$id and .transport=="binary_stream"' "$scratch/$operation.json" >/dev/null
    # Completion alone is insufficient: the invocation must report recovery.
    sed -n '/resuming/p' "$scratch/$operation.stderr" >"$scratch/$operation-recovery.txt"
    test -s "$scratch/$operation-recovery.txt"
    jq -n --arg operation "$operation" --argjson elapsed "$((finished-started))" \
        '{operation:$operation,interrupted:true,elapsed_ms:$elapsed}' | tee "$scratch/$operation-measurement.json"
done
cmp "$scratch/source.bin" "$scratch/received.bin"
w delete >/dev/null
trap - EXIT
echo "Active transfer recovery passed ($engine); artifacts retained in $scratch"
