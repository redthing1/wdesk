#!/usr/bin/env bash
set -Eeuo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
engine="${1:-native}"
scratch="$(mktemp -d /tmp/wdesk-windows-processes.XXXXXXXX)"
session="processes-$engine-$$"
unset WDESK_DESCRIPTOR
w() { "$bin" --session "$session" --json "$@"; }
cleanup() { w stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'echo "Process acceptance failed at line $LINENO; artifacts: $scratch" >&2' ERR
w open --image "${WDESK_TEST_IMAGE:-windows-lite}" --engine "$engine" --memory 2048 --cpus 2 --offline --timeout 240 >"$scratch/open.json"
w capabilities | jq -e '.process_retention.bounded and .process_retention.active_limit == 128 and .process_retention.completed_limit == 128 and .process_retention.completed_seconds == 600' >/dev/null
w import tests/process-probe.ps1 workspace/process-probe.ps1 >/dev/null
w import tests/control-probe.ps1 workspace/control-probe.ps1 >/dev/null
for architecture in x64 x86; do
    executable=powershell.exe
    if [ "$architecture" = x86 ]; then executable='C:\Windows\SysWOW64\WindowsPowerShell\v1.0\powershell.exe'; fi
    started="$(w process start --timeout 120 -- "$executable" -NoProfile -ExecutionPolicy Bypass -File 'C:\ProgramData\wdesk\workspace\process-probe.ps1')"
    id="$(jq -er .id <<<"$started")"
    w process wait "$id" --timeout 130 >"$scratch/$architecture.json"
    jq -e '.exit_code == 0 and .output_complete and (.output | fromjson | .processes == 160 and .independent_expiry and .descendants_retired)' "$scratch/$architecture.json" >/dev/null
    w process forget "$id" | jq -e '.forgotten' >/dev/null
    if w process status "$id" >"$scratch/$architecture-forgotten.json" 2>"$scratch/$architecture-forgotten.stderr"; then echo 'Forgotten receipt survived' >&2; exit 1; fi
    started="$(w process start --timeout 90 -- "$executable" -NoProfile -ExecutionPolicy Bypass -File 'C:\ProgramData\wdesk\workspace\control-probe.ps1')"
    id="$(jq -er .id <<<"$started")"
    w process wait "$id" --timeout 100 >"$scratch/control-$architecture.json"
    jq -e '.exit_code == 0 and .output_complete and (.output | fromjson | .requests == 1000 and .old_readability_race and .utf8_boundary and .queued_eof)' "$scratch/control-$architecture.json" >/dev/null
    w process forget "$id" >/dev/null
done
started="$(w process start --timeout 30 -- powershell.exe -NoProfile -Command 'Start-Sleep -Seconds 20')"
id="$(jq -er .id <<<"$started")"
if w process forget "$id" >/dev/null 2>&1; then echo 'Active process was forgotten' >&2; exit 1; fi
if w process wait "$id" --timeout 0 >/dev/null 2>&1; then echo 'Wait deadline was ignored' >&2; exit 1; fi
w process kill "$id" >/dev/null
w process wait "$id" --timeout 10 | jq -e '.running == false and .phase == "completed" and .output_complete' >/dev/null
w process forget "$id" >/dev/null
w delete >/dev/null
trap - EXIT
echo "Windows processes passed ($engine); artifacts retained in $scratch"
