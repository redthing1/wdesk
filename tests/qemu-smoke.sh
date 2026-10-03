#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
scratch="$(mktemp -d /tmp/wdesk-smoke.XXXXXXXX)"
# Exercise startup even when an upgrade has unlinked the running CLI binary.
cp "$bin" "$scratch/unlinked-wdesk"
exec 9<"$scratch/unlinked-wdesk"
rm "$scratch/unlinked-wdesk"
bin="/proc/$$/fd/9"
export WDESK_HOME="$scratch/state"
session="smoke-$$"
cleanup() { "$bin" --session "$session" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
qemu-img create -f qcow2 "$scratch/blank.qcow2" 128M >/dev/null
"$bin" image import "$scratch/blank.qcow2" --name blank --compress >/dev/null
"$bin" --session "$session" open --image blank --no-wait --offline --memory 512 >/dev/null
"$bin" --session "$session" --json see --output "$scratch/bios.png" >"$scratch/observation.json"
jq -e '.geometry.width > 0 and .geometry.height > 0 and .cursor_included == false' "$scratch/observation.json" >/dev/null
"$bin" --session "$session" --json capabilities | jq -e '.guest.ready == false and .console.physical_input == true' >/dev/null
descriptor="$WDESK_HOME/sessions/$session/client.json"
endpoint="$(jq -r .endpoint "$descriptor")"
token="$(jq -r .token "$descriptor")"
epoch="$(jq -r .epoch "$descriptor")"
code="$(curl -s -o /dev/null -w '%{http_code}' "$endpoint/v1/health")"
test "$code" = 401
jq -n --arg epoch "$epoch" '{protocol:1,epoch:$epoch,request_id:"smoke-input",expected_input_generation:0,actions:[{type:"key",keys:["ENTER"]}]}' >"$scratch/batch.json"
"$bin" --session "$session" --json batch "$scratch/batch.json" >"$scratch/first.json"
"$bin" --session "$session" --json batch "$scratch/batch.json" >"$scratch/retry.json"
cmp "$scratch/first.json" "$scratch/retry.json"
jq '.request_id="stale"' "$scratch/batch.json" >"$scratch/stale.json"
if "$bin" --session "$session" batch "$scratch/stale.json" >/dev/null 2>&1; then echo 'stale input accepted' >&2; exit 1; fi
"$bin" --session "$session" --json reset --no-wait >/dev/null
new_epoch="$(jq -r .epoch "$descriptor")"
test "$new_epoch" != "$epoch"
new_endpoint="$(jq -r .endpoint "$descriptor")"
code="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "$new_endpoint/v1/health")"
test "$code" = 401
"$bin" --session "$session" delete >/dev/null
trap - EXIT
echo "QEMU smoke passed; artifacts retained in $scratch"
