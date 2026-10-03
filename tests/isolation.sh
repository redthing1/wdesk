#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
image="${WDESK_TEST_IMAGE:-windows-lite}"
scratch="$(mktemp -d /tmp/wdesk-isolation.XXXXXXXX)"
a="isolation-a-$$"
b="isolation-b-$$"
w() { "$bin" --session "$1" --json "${@:2}"; }
cleanup() { w "$a" stop >/dev/null 2>&1 || true; w "$b" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
w "$a" open --image "$image" --engine native --offline --timeout 240 >"$scratch/a.json" &
pa=$!
w "$b" open --image "$image" --engine "${1:-podman}" --offline --timeout 240 >"$scratch/b.json" &
pb=$!
wait "$pa"
wait "$pb"
w "$a" clipboard set 'isolated guest A' >/dev/null
w "$b" clipboard set 'isolated guest B' >/dev/null
w "$a" clipboard get | jq -e '.text == "isolated guest A"' >/dev/null
w "$b" clipboard get | jq -e '.text == "isolated guest B"' >/dev/null
printf 'only A\n' >"$scratch/only-a.txt"
w "$a" import "$scratch/only-a.txt" workspace/only-a.txt >/dev/null
if w "$b" export workspace/only-a.txt "$scratch/leak.txt" >/dev/null 2>&1; then echo 'file leaked between overlays' >&2; exit 1; fi
root="${WDESK_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/wdesk}"
endpoint="$(jq -r .endpoint "$root/sessions/$b/client.json")"
token="$(jq -r .token "$root/sessions/$a/client.json")"
test "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "$endpoint/v1/health")" = 401
w "$a" delete >/dev/null
w "$b" delete >/dev/null
trap - EXIT
echo "Isolation passed (native + ${1:-podman}); artifacts retained in $scratch"
