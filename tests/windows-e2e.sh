#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
image="${WDESK_TEST_IMAGE:-windows-lite}"
engine="${1:-native}"
session="e2e-$engine-$$"
scratch="$(mktemp -d /tmp/wdesk-windows-e2e.XXXXXXXX)"
w() { "$bin" --session "$session" --json "$@"; }
cleanup() { w stop >/dev/null 2>&1 || true; }
failed() {
    echo "Windows E2E failed ($engine) at line $1; artifacts: $scratch" >&2
    w see --output "$scratch/failure.png" >"$scratch/failure.json" 2>/dev/null || true
    w clipboard get >"$scratch/failure-clipboard.json" 2>/dev/null || true
}
trap cleanup EXIT
trap 'failed "$LINENO"' ERR
w open --image "$image" --engine "$engine" --memory "${WDESK_TEST_MEMORY:-4096}" --cpus "${WDESK_TEST_CPUS:-2}" --offline --timeout 240 >"$scratch/open.json"
w see --output "$scratch/desktop.png" >"$scratch/observation.json"
if [ -n "${WDESK_TEST_VERSION:-}" ]; then
    w status | jq -e --arg version "$WDESK_TEST_VERSION" '.guest.windows.version == $version' >/dev/null
fi
w capabilities | jq -e '.guest.ready and .guest.processes and .guest.files' >/dev/null
deadline=$((SECONDS + 60))
while ! w capabilities | jq -e '.guest.transport == "tcp_guestfwd"' >/dev/null; do
    test "$SECONDS" -lt "$deadline"
    sleep 2
done
text='Hello wdesk — café 日本語 🐈'
w clipboard set "$text" >/dev/null
w clipboard get | jq -e --arg text "$text" '.text == $text' >/dev/null
printf 'chunked transfer\n' >"$scratch/input.txt"
head -c 131073 /dev/urandom >"$scratch/payload.bin"
w import "$scratch/payload.bin" workspace/payload.bin >"$scratch/import.json"
w export workspace/payload.bin "$scratch/export.bin" >/dev/null
cmp "$scratch/payload.bin" "$scratch/export.bin"
if w export workspace/../outside.txt "$scratch/bad" >/dev/null 2>&1; then echo 'traversal accepted' >&2; exit 1; fi
if w import "$scratch/input.txt" workspace/CON >/dev/null 2>&1; then echo 'device name accepted' >&2; exit 1; fi
p="$(w process start --timeout 10 -- cmd.exe /d /c 'echo wdesk-process-test')"
pid="$(jq -r .id <<<"$p")"
w process wait "$pid" --timeout 15 >"$scratch/process.json"
jq -e '.exit_code == 0 and (.output | contains("wdesk-process-test"))' "$scratch/process.json" >/dev/null
p="$(w process start --timeout 20 -- 'C:\Windows\SysWOW64\WindowsPowerShell\v1.0\powershell.exe' -NoProfile -Command 'if ([Environment]::Is64BitProcess) { exit 1 }; Add-Type -AssemblyName System.Windows.Forms,UIAutomationClient; [Console]::Write("wdesk-x86")')"
pid="$(jq -r .id <<<"$p")"
w process wait "$pid" --timeout 25 | jq -e '.exit_code == 0 and (.output | contains("wdesk-x86"))' >/dev/null
if [ "${WDESK_TEST_COMPACT:-0}" = 1 ]; then
    p="$(w process start --timeout 20 -- powershell.exe -NoProfile -Command '$ErrorActionPreference="Stop"; $state=(& compact.exe /compactos:query | Out-String); if ($state -notmatch "system is in the Compact state") { throw $state }; [Console]::Write($state)')"
    pid="$(jq -r .id <<<"$p")"
    w process wait "$pid" --timeout 25 | jq -e '.exit_code == 0 and (.output | ascii_downcase | contains("system is in the compact state"))' >/dev/null
fi
p="$(w process start --timeout 2 -- powershell.exe -NoProfile -Command 'Start-Sleep -Seconds 30')"
pid="$(jq -r .id <<<"$p")"
w process wait "$pid" --timeout 10 | jq -e '.timed_out and (.running == false)' >/dev/null
p="$(w process start --timeout 15 -- powershell.exe -NoProfile -Command '[Console]::Write(("x" * 100000))')"
pid="$(jq -r .id <<<"$p")"
w process wait "$pid" --timeout 20 | jq -e '.exit_code == 0 and .output_truncated and (.output | length == 65536)' >/dev/null
w import tests/input-probe.ps1 workspace/input-probe.ps1 >/dev/null
w launch -- powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -WindowStyle Hidden -File 'C:\ProgramData\wdesk\workspace\input-probe.ps1' >/dev/null
deadline=$((SECONDS + 30))
while true; do
    w windows >"$scratch/windows.json"
    window="$(jq -r '.windows[] | select(.title == "wdesk input probe") | .id' "$scratch/windows.json" | head -1)"
    [ -z "$window" ] || break
    test "$SECONDS" -lt "$deadline"
    sleep 1
done
w windows --focus "$window" >/dev/null
w type "$text" >/dev/null
w clipboard set '' >/dev/null
w key CTRL+HOME >/dev/null
w key CTRL+SHIFT+END >/dev/null
w key CTRL+C >/dev/null
deadline=$((SECONDS + 10))
until w clipboard get | jq -e --arg text "$text" '.text == $text' >/dev/null; do
    test "$SECONDS" -lt "$deadline"
    sleep 0.2
done
w a11y >"$scratch/a11y.json"
jq -e '.nodes | length > 0' "$scratch/a11y.json" >/dev/null
jq -e 'all(.nodes[]; (.bounds == null) or (.bounds.width >= 0 and .bounds.height >= 0))' "$scratch/a11y.json" >/dev/null
w see --output "$scratch/input-probe.png" >"$scratch/after.json"
generation="$(jq -r .input_generation "$scratch/after.json")"
w key ESC >/dev/null
if w click 10 10 --generation "$generation" >/dev/null 2>&1; then echo 'stale generation accepted' >&2; exit 1; fi
if [ "$engine" = native ]; then
    helper="$(w status | jq -r .guest.helper_id)"
    w launch -- shutdown.exe /r /f /t 0 >/dev/null
    deadline=$((SECONDS + 240))
    while [ "$(w status | jq -r .guest.helper_id)" = "$helper" ] || ! w status | jq -e '.guest.helper_id != null' >/dev/null; do
        test "$SECONDS" -lt "$deadline"
        sleep 2
    done
    w wait --timeout 60 >/dev/null
    if w windows --focus "$window" >/dev/null 2>&1; then echo 'stale window id accepted after reboot' >&2; exit 1; fi
fi
epoch="$(jq -r .epoch "$scratch/after.json")"
w reset --timeout 240 >"$scratch/reset.json"
test "$(jq -r .epoch "$scratch/reset.json")" != "$epoch"
if w export workspace/payload.bin "$scratch/reset-leak.bin" >/dev/null 2>&1; then echo 'overlay survived reset' >&2; exit 1; fi
w delete >/dev/null
trap - EXIT
echo "Windows E2E passed ($engine); artifacts retained in $scratch"
