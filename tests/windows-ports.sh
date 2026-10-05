#!/usr/bin/env bash
set -Eeuo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
image="${WDESK_TEST_IMAGE:-windows-lite}"
engine="${1:-native}"
session="ports-$engine-$$"
scratch="$(mktemp -d /tmp/wdesk-windows-ports.XXXXXXXX)"
port="${WDESK_TEST_PORT:-$((30000 + $$ % 20000))}"
other=$((port + 1))
token="ports-$engine-$$"
w() { "$bin" --session "$session" --json "$@"; }
cleanup() { w stop >/dev/null 2>&1 || true; }
failed() {
    echo "Port acceptance failed at line $1; artifacts: $scratch" >&2
    w see --output "$scratch/failure.png" >"$scratch/failure.json" 2>/dev/null || true
    w status >"$scratch/failure-status.json" 2>/dev/null || true
}
trap cleanup EXIT
trap 'failed "$LINENO"' ERR
w open --image "$image" --engine "$engine" --memory "${WDESK_TEST_MEMORY:-2048}" --offline --forward "$port:8080" --forward "$other:8081" --timeout 240 >"$scratch/open.json"
w port list | jq -e --arg host "127.0.0.1:$port" '.forwards | length == 2 and .[0].host == $host' >/dev/null
w status | jq -e '.forwards | length == 2' >/dev/null
w connect "$scratch/client.json" >/dev/null
descriptor="$scratch/client.json"
if WDESK_DESCRIPTOR="$descriptor" w port remove "$port" >"$scratch/agent.json" 2>"$scratch/agent.err"; then echo 'Agent changed forwarding' >&2; exit 1; fi
if w port remove "$port" >"$scratch/live.json" 2>"$scratch/live.err"; then echo 'Live change accepted' >&2; exit 1; fi
if w open --forward "$port:8082" --no-wait >"$scratch/mismatch.json" 2>"$scratch/mismatch.err"; then echo 'Saved mapping silently replaced' >&2; exit 1; fi
approve_probe() {
    # Observe the secure desktop before pressing keys; boot/engine timing varies.
    deadline=$((SECONDS + 30))
    until w status | jq -e '.guest.interactive == false' >/dev/null; do
        if [ "$(curl --noproxy '*' -fsS --max-time 1 "http://127.0.0.1:$1" 2>/dev/null || true)" = "$token" ]; then return; fi
        test "$SECONDS" -lt "$deadline"
        sleep 1
    done
    w key LEFT >/dev/null
    w key ENTER >/dev/null
}
start_probe() {
    w import tests/port-probe.ps1 workspace/port-probe.ps1 >/dev/null
    for guest_port in 8080 8081; do
        # Only this disposable fixture is elevated; normal helper integrity stays unchanged.
        w process start --timeout 620 -- powershell.exe -NoProfile -Command "Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File C:\ProgramData\wdesk\workspace\port-probe.ps1 -Port $guest_port -Token $token' -Wait" >"$scratch/probe-$guest_port.json"
        host_port="$port"; [ "$guest_port" = 8080 ] || host_port="$other"
        approve_probe "$host_port"
        deadline=$((SECONDS + 60))
        until [ "$(curl --noproxy '*' -fsS --max-time 3 "http://127.0.0.1:$host_port" 2>/dev/null || true)" = "$token" ]; do
            test "$SECONDS" -lt "$deadline"
            sleep 1
        done
        curl --noproxy '*' -fsS --max-time 3 -D "$scratch/headers-$guest_port.txt" "http://127.0.0.1:$host_port" >"$scratch/response-$guest_port.txt"
        rg -q '^X-Wdesk-Peer: 10[.]0[.]2[.]2\r?$' "$scratch/headers-$guest_port.txt"
    done
}
start_probe
# No ordinary external routing, despite explicit local forwards.
p="$(w process start --timeout 10 -- powershell.exe -NoProfile -Command '$c=New-Object Net.Sockets.TcpClient; try { $a=$c.BeginConnect("1.1.1.1",443,$null,$null); if($a.AsyncWaitHandle.WaitOne(2000)) { $c.EndConnect($a); exit 1 }; exit 0 } catch { exit 0 } finally { $c.Dispose() }')"
w process wait "$(jq -r .id <<<"$p")" --timeout 15 | jq -e '.exit_code == 0' >/dev/null
# A second session must fail clearly rather than silently choose another port.
collision="ports-collision-$engine-$$"
if "$bin" --session "$collision" open --image "$image" --engine "$engine" --forward "$port:8080" --no-wait >"$scratch/collision.json" 2>"$scratch/collision.err"; then echo 'Occupied port accepted' >&2; exit 1; fi
rg -q "cannot bind host TCP port 127.0.0.1:$port" "$scratch/collision.err"
"$bin" --session "$collision" delete >/dev/null
w stop >/dev/null
if curl --noproxy '*' -fsS --max-time 2 "http://127.0.0.1:$port" >/dev/null 2>&1; then echo 'Listener survived stop' >&2; exit 1; fi
w open --timeout 240 >/dev/null
start_probe
w reset --timeout 240 >"$scratch/reset.json"
w port list | jq -e '.forwards | length == 2' >/dev/null
start_probe
w stop >/dev/null
w port remove "$port" >/dev/null
if w port add "$other:8080" >"$scratch/duplicate.json" 2>"$scratch/duplicate.err"; then echo 'Duplicate host port accepted' >&2; exit 1; fi
w port list | jq -e '.forwards | length == 1' >/dev/null
w open --timeout 240 >/dev/null
w process start --timeout 620 -- powershell.exe -NoProfile -Command "Start-Process powershell.exe -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File C:\ProgramData\wdesk\workspace\port-probe.ps1 -Port 8081 -Token $token' -Wait" >"$scratch/probe-remaining.json"
approve_probe "$other"
deadline=$((SECONDS + 60))
until [ "$(curl --noproxy '*' -fsS --max-time 3 "http://127.0.0.1:$other" 2>/dev/null || true)" = "$token" ]; do test "$SECONDS" -lt "$deadline"; sleep 1; done
if curl --noproxy '*' -fsS --max-time 2 "http://127.0.0.1:$port" >/dev/null 2>&1; then echo 'Removed mapping reachable' >&2; exit 1; fi
w delete >/dev/null
if curl --noproxy '*' -fsS --max-time 2 "http://127.0.0.1:$other" >/dev/null 2>&1; then echo 'Listener survived delete' >&2; exit 1; fi
trap - EXIT
echo "Windows port acceptance passed ($engine); artifacts retained in $scratch"
