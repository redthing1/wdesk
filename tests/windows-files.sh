#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
image="${WDESK_TEST_IMAGE:-windows-lite}"
engine="${1:-native}"
case "$engine" in native|docker|podman) ;; *) echo 'Expected native, docker, or podman' >&2;exit 1;; esac
for tool in curl jq sha256sum head tail cmp timeout; do command -v "$tool" >/dev/null; done
unset WDESK_DESCRIPTOR
session="files-$engine-$$"
scratch="$(mktemp -d /tmp/wdesk-windows-files.XXXXXXXX)"
w() { "$bin" --session "$session" --json "$@"; }
cleanup() { w stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
echo "Windows file test artifacts: $scratch" >&2
w open --image "$image" --engine "$engine" --offline --memory "${WDESK_TEST_MEMORY:-2048}" --cpus 2 --timeout 240 >"$scratch/open.json"
w capabilities | jq -e '.file_transfer.binary_stream and .file_transfer.atomic_upload and (.file_transfer.mount_required==false)' >/dev/null
w connect "$scratch/client.json" >/dev/null
endpoint=$(jq -r .endpoint "$scratch/client.json")
token=$(jq -r .token "$scratch/client.json")
epoch=$(jq -r .epoch "$scratch/client.json")
helper=$(w status | jq -r .guest.helper_id)
identity=$(jq -nc --arg epoch "$epoch" --arg helper "$helper" '{epoch:$epoch,helper_id:$helper}')
uuid() { read -r transfer_id </proc/sys/kernel/random/uuid;printf '%s' "$transfer_id"; }
api() { curl --silent --show-error --fail-with-body --max-time 40 -H "Authorization: Bearer $token" "$@"; }
status() { api "$endpoint/v1/transfers/$1?epoch=$epoch&helper_id=$helper"; }
action() { api -H 'Content-Type: application/json' -d "$identity" "$endpoint/v1/transfers/$1/$2"; }
begin() {
    jq -nc --arg id "$1" --arg direction "$2" --arg path "$3" --argjson size "$4" --arg hash "$5" --arg epoch "$epoch" --arg helper "$helper" \
        '{id:$id,epoch:$epoch,helper_id:$helper,direction:$direction,path:$path,size:$size,sha256:$hash}' |
        api -H 'Content-Type: application/json' --data-binary @- "$endpoint/v1/transfers"
}
wait_phase() {
    local deadline=$((SECONDS+50))
    while true; do
        status "$1" >"$scratch/status.json"
        jq -e --arg phase "$2" '.phase==$phase and (.active==false)' "$scratch/status.json" >/dev/null && return
        [ "$SECONDS" -lt "$deadline" ] || { cat "$scratch/status.json" >&2;return 1; }
        sleep 0.2
    done
}
head -c 8388608 /dev/urandom >"$scratch/source.bin"
hash=$(sha256sum "$scratch/source.bin" | cut -d' ' -f1)
id=$(uuid)
begin "$id" upload workspace/resume.bin 8388608 "$hash" >"$scratch/begin.json"
begin "$id" upload workspace/resume.bin 8388608 "$hash" >"$scratch/duplicate-begin.json"
url="$endpoint/v1/transfers/$id/data?epoch=$epoch&helper_id=$helper"
api --max-time 0.8 --limit-rate 1M -T "$scratch/source.bin" "$url&offset=0&length=8388608" >"$scratch/partial.json" 2>"$scratch/partial.stderr" &
sender=$!
sleep 0.2
timeout 10 "$bin" --session "$session" --json see --output "$scratch/during-upload.png" >"$scratch/during-upload.json"
timeout 10 "$bin" --session "$session" --json clipboard get >"$scratch/during-upload-clipboard.json"
if wait "$sender"; then echo 'Expected partial upload' >&2;exit 1;fi
action "$id" pause >"$scratch/pause.json"
wait_phase "$id" ready
offset=$(jq -r .offset "$scratch/status.json")
test "$offset" -gt 0 && test "$offset" -lt 8388608
# A changed source must not cancel the original resumable transfer.
if w import tests/input-probe.ps1 workspace/resume.bin --resume "$id" >"$scratch/wrong-source.json" 2>"$scratch/wrong-source.stderr"; then exit 1;fi
status "$id" | jq -e '.phase=="ready"' >/dev/null
w import "$scratch/source.bin" workspace/resume.bin --resume "$id" >"$scratch/resumed.json"
wait_phase "$id" committed
action "$id" commit >"$scratch/duplicate-commit.json"
w export workspace/resume.bin "$scratch/export.bin" >"$scratch/export.json"
cmp "$scratch/source.bin" "$scratch/export.bin"
unicode_path='workspace/データ – café.bin'
w import "$scratch/source.bin" "$unicode_path" >/dev/null
w import tests/input-probe.ps1 "$unicode_path" >/dev/null
w export "$unicode_path" "$scratch/unicode.bin" >/dev/null
cmp tests/input-probe.ps1 "$scratch/unicode.bin"
: >"$scratch/empty.bin"
w import "$scratch/empty.bin" workspace/empty.bin >/dev/null
w export workspace/empty.bin "$scratch/empty-export.bin" >/dev/null
cmp "$scratch/empty.bin" "$scratch/empty-export.bin"
# Stable source handles and explicit receive offsets support interrupted reads.
read_id=$(uuid)
begin "$read_id" download workspace/resume.bin 0 '' >"$scratch/download-begin.json"
wait_phase "$read_id" ready
read_url="$endpoint/v1/transfers/$read_id/data?epoch=$epoch&helper_id=$helper"
if api --max-time 0.8 --limit-rate 1M "$read_url&offset=0&length=8388608" >"$scratch/received.bin" 2>"$scratch/download-partial.stderr"; then exit 1;fi
action "$read_id" pause >/dev/null
wait_phase "$read_id" ready
received=$(wc -c <"$scratch/received.bin")
test "$received" -gt 0 && test "$received" -lt 8388608
api "$read_url&offset=$received&length=$((8388608-received))" >>"$scratch/received.bin"
cmp "$scratch/source.bin" "$scratch/received.bin"
wait_phase "$read_id" ready
action "$read_id" commit >/dev/null
wait_phase "$read_id" committed
# Failed checksum verification must leave an existing destination intact.
w import tests/input-probe.ps1 workspace/preserved.bin >/dev/null
bad=$(uuid)
begin "$bad" upload workspace/preserved.bin 3 "$(printf '%064d' 0)" >/dev/null
api -X PUT --data-binary abc "$endpoint/v1/transfers/$bad/data?epoch=$epoch&helper_id=$helper&offset=0&length=3" >/dev/null
action "$bad" commit >/dev/null
wait_phase "$bad" failed
w export workspace/preserved.bin "$scratch/preserved.bin" >/dev/null
cmp tests/input-probe.ps1 "$scratch/preserved.bin"
# A stable download also prevents replacing its source until it is released.
held=$(uuid)
begin "$held" download workspace/preserved.bin 0 '' >/dev/null
wait_phase "$held" ready
locked=$(uuid)
abc_hash=$(printf abc | sha256sum | cut -d' ' -f1)
begin "$locked" upload workspace/preserved.bin 3 "$abc_hash" >/dev/null
api -X PUT --data-binary abc "$endpoint/v1/transfers/$locked/data?epoch=$epoch&helper_id=$helper&offset=0&length=3" >/dev/null
action "$locked" commit >/dev/null
wait_phase "$locked" failed
action "$held" abort >/dev/null
wait_phase "$held" cancelled
w export workspace/preserved.bin "$scratch/locked-preserved.bin" >/dev/null
cmp tests/input-probe.ps1 "$scratch/locked-preserved.bin"
cancel=$(uuid)
begin "$cancel" upload workspace/cancelled.bin 8388608 "$hash" >/dev/null
api --max-time 0.8 --limit-rate 1M -T "$scratch/source.bin" "$endpoint/v1/transfers/$cancel/data?epoch=$epoch&helper_id=$helper&offset=0&length=8388608" >"$scratch/cancel-partial.json" 2>"$scratch/cancel-partial.stderr" || true
w transfer cancel "$cancel" >"$scratch/cancel.json"
wait_phase "$cancel" cancelled
if w export workspace/cancelled.bin "$scratch/cancelled.bin" >/dev/null 2>&1; then exit 1;fi
w delete >/dev/null
trap - EXIT
echo "Windows file tests passed ($engine); artifacts retained in $scratch"
