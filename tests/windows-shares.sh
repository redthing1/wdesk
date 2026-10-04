#!/usr/bin/env bash
set -Eeuo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
engine="${1:-native}"
scratch="$(mktemp -d /tmp/wdesk-windows-shares.XXXXXXXX)"
session="shares-$engine-$(date +%s)-$$"
other="$session-other"
unset WDESK_DESCRIPTOR
w() { "$bin" --session "$session" --json "$@"; }
b() { "$bin" --session "$other" --json "$@"; }
cleanup() { w stop >/dev/null 2>&1 || true; b stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'echo "Share acceptance failed at line $LINENO; artifacts: $scratch" >&2' ERR
mkdir -p "$scratch/source" "$scratch/output"
printf 'owner granted source\n' >"$scratch/source/source.txt"
printf 'not granted\n' >"$scratch/outside.txt"
ln -s "$scratch/outside.txt" "$scratch/source/outside-link.txt"
ln -s "$scratch/outside.txt" "$scratch/output/outside-link.txt"
w open --image "${WDESK_TEST_IMAGE:-windows-lite}" --engine "$engine" --memory 2048 --cpus 2 --offline --timeout 240 >"$scratch/initial-open.json"
w status | jq -e '.guest.features.host_shares == 1' >/dev/null
w stop >/dev/null
w share add source "$scratch/source" >"$scratch/grant-source.json"
w share add output "$scratch/output" --write >"$scratch/grant-output.json"
w open --timeout 240 >"$scratch/open.json"
attached() {
    local deadline=$((SECONDS+90))
    until w capabilities >"$scratch/capabilities.json" && jq -e '.shares.attached' "$scratch/capabilities.json" >/dev/null; do
        test "$SECONDS" -lt "$deadline"
        sleep 1
    done
    jq -e '.shares.backend == "smb" and (.shares.host_writes_rollback == false)' "$scratch/capabilities.json" >/dev/null
}
attached
w connect "$scratch/client.json" >/dev/null
if WDESK_DESCRIPTOR="$scratch/client.json" "$bin" share add denied "$scratch/source" --write >/dev/null 2>&1; then echo 'agent changed grants' >&2; exit 1; fi
if w share remove source >/dev/null 2>&1; then echo 'live grant change accepted' >&2; exit 1; fi
if rg -q 'password|username|"engine"|"shares"' "$scratch/client.json"; then echo 'owner state in descriptor' >&2; exit 1; fi
w import tests/share-probe.ps1 workspace/share-probe.ps1 >/dev/null
probe() {
    local label="$1" executable="$2" expected="$3" started id
    started="$(w process start --timeout 40 -- "$executable" -NoProfile -ExecutionPolicy Bypass -File 'C:\ProgramData\wdesk\workspace\share-probe.ps1' -Expected "$expected")"
    id="$(jq -er .id <<<"$started")"
    w process wait "$id" --timeout 50 >"$scratch/$label.json"
    jq -e --arg unicode '日本語 🐈.txt' '.exit_code == 0 and (.timed_out == false) and (.output_truncated == false) and (.output | fromjson | .read_only and .locking and .symlink_denied and .unicode_rename and .unicode_name == $unicode)' "$scratch/$label.json" >/dev/null
}
probe normal powershell.exe 'owner granted source'
probe x86 'C:\Windows\SysWOW64\WindowsPowerShell\v1.0\powershell.exe' 'owner granted source'
test ! -e "$scratch/source/forbidden.txt"
test -f "$scratch/output/renamed.txt"
printf 'live host edit\n' >"$scratch/source/source.txt"
probe live-edit powershell.exe 'live host edit'
# An offline guest without a grant cannot reach the other's private endpoint.
b open --image "${WDESK_TEST_IMAGE:-windows-lite}" --engine "$engine" --memory 2048 --cpus 2 --offline --timeout 240 >"$scratch/other-open.json"
b capabilities | jq -e '.shares.backend == null and (.shares.grants | length == 0)' >/dev/null
started="$(b process start --timeout 30 -- powershell.exe -NoProfile -Command 'try { [IO.File]::ReadAllText("\\10.0.2.102\source\source.txt") | Out-Null; exit 1 } catch { exit 0 }')"
b process wait "$(jq -er .id <<<"$started")" --timeout 40 | jq -e '.exit_code == 0 and (.timed_out == false)' >/dev/null
b delete >/dev/null
epoch="$(jq -er .epoch "$scratch/capabilities.json")"
w reset --timeout 240 >"$scratch/reset.json"
attached
test "$(jq -er .epoch "$scratch/capabilities.json")" != "$epoch"
test -f "$scratch/output/renamed.txt"
w import tests/share-probe.ps1 workspace/share-probe.ps1 >/dev/null
probe after-reset powershell.exe 'live host edit'
w see --output "$scratch/desktop.png" >/dev/null
w delete >/dev/null
test -f "$scratch/source/source.txt"
test -f "$scratch/output/renamed.txt"
trap - EXIT
echo "Windows shares passed ($engine); artifacts retained in $scratch"
