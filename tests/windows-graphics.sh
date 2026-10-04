#!/usr/bin/env bash
set -Eeuo pipefail
cd "$(dirname "$0")/.."
bin="${WDESK_BIN:-$PWD/target/debug/wdesk}"
engine="${1:-native}"
scratch="$(mktemp -d /tmp/wdesk-windows-graphics.XXXXXXXX)"
session="${WDESK_TEST_SESSION:-graphics-$engine-$(date +%s)-$$}"
remote="workspace/graphics-test-$$"
guest="C:\\ProgramData\\wdesk\\workspace\\graphics-test-$$"
unset WDESK_DESCRIPTOR
w() { "$bin" --session "$session" --json "$@"; }
cleanup() { if [ -z "${WDESK_TEST_SESSION:-}" ]; then w stop >/dev/null 2>&1 || true; fi; }
trap cleanup EXIT
trap 'echo "Graphics acceptance failed at line $LINENO; artifacts: $scratch" >&2' ERR
if [ -z "${WDESK_TEST_SESSION:-}" ]; then
    w open --image "${WDESK_TEST_IMAGE:-windows-lite}" --engine "$engine" \
        --memory "${WDESK_TEST_MEMORY:-2048}" --cpus 2 --offline --timeout 240 >"$scratch/open.json"
fi
w capabilities | jq -e '.graphics.preset_setup' >/dev/null
bash tests/build-graphics.sh "$scratch/fixtures"
wait_process() {
    local id="$1" label="$2"
    w process wait "$id" --timeout 60 >"$scratch/$label.json"
    jq -e '.exit_code == 0 and (.timed_out == false) and (.output_truncated == false)' "$scratch/$label.json" >/dev/null
}
render() {
    local arch="$1" file="$2" api="$3" windowed="$4"
    shift 4
    local started id label="$api-$arch"
    started="$(w graphics run "$remote/$arch/$file" --apis gl,gles,vk --timeout 45 -- "$@")"
    printf '%s\n' "$started" >"$scratch/$label-start.json"
    id="$(jq -er .id <<<"$started")"
    if [ "$windowed" = yes ]; then
        local deadline=$((SECONDS + 30))
        until w process output "$id" | jq -e '.output | contains("\"presented\":true")' >/dev/null; do
            test "$SECONDS" -lt "$deadline"
            w process status "$id" | jq -e .running >/dev/null
            sleep 0.05
        done
        sleep 0.25
        w see --output "$scratch/$label.png" >"$scratch/$label-capture.json"
        w process status "$id" | jq -e .running >/dev/null
    fi
    wait_process "$id" "$label"
    jq -e --arg api "$api" '.output | fromjson | .api == $api and .pixel == [64,128,191,255]' "$scratch/$label.json" >/dev/null
}
for arch in x64 x86; do
    w graphics install --arch "$arch" --apis gl,gles,vk >"$scratch/install-$arch.json"
    w graphics install --arch "$arch" --apis gl,gles,vk >"$scratch/reinstall-$arch.json"
    jq -e '.uploaded_bytes <= 8192 and all(.files[] | select(.name | endswith(".txt") | not); .reused)' "$scratch/reinstall-$arch.json" >/dev/null
    for kind in probe egl vulkan; do
        w import "$scratch/fixtures/graphics-$kind-$arch.exe" "$remote/$arch/graphics-$kind-$arch.exe" >/dev/null
    done
    w graphics prepare "$remote/$arch/graphics-probe-$arch.exe" --apis gl,gles,vk >"$scratch/prepare-$arch.json"
    jq -e --arg arch "$arch" '.architecture == $arch and .created == 8' "$scratch/prepare-$arch.json" >/dev/null
    w graphics prepare "$remote/$arch/graphics-probe-$arch.exe" --apis gl,gles,vk | jq -e '.created == 0 and .reused == 8' >/dev/null
    for api in gl d3d9 d3d10 d3d11 d3d12; do
        windowed=no; if [ "$api" = gl ] || [ "$api" = d3d9 ]; then windowed=yes; fi
        render "$arch" "graphics-probe-$arch.exe" "$api" "$windowed" "$api"
    done
    render "$arch" "graphics-egl-$arch.exe" gles yes
    render "$arch" "graphics-vulkan-$arch.exe" vk yes
done
w import tests/graphics-env.cs "$remote/managed/graphics-env.cs" >/dev/null
w import tests/graphics-build-managed.ps1 "$remote/managed/build.ps1" >/dev/null
started="$(w process start --timeout 30 -- powershell.exe -NoProfile -ExecutionPolicy Bypass \
    -File "$guest\managed\build.ps1" -Directory "$guest\managed")"
wait_process "$(jq -er .id <<<"$started")" managed-build
for platform in anycpu x64 x86 anycpu32bitpreferred; do
    arch=x64; if [ "$platform" = x86 ] || [ "$platform" = anycpu32bitpreferred ]; then arch=x86; fi
    w graphics prepare "$remote/managed/$platform/probe.exe" --apis gl,gles,vk | jq -e --arg arch "$arch" '.architecture == $arch' >/dev/null
    started="$(w graphics run "$remote/managed/$platform/probe.exe" --apis gl,gles,vk \
        -- 'Hello 日本語 🐈' '$name; literal' 'a"b\c')"
    wait_process "$(jq -er .id <<<"$started")" "managed-$platform"
    jq -e --arg arch "$arch" '.output | fromjson | .architecture == $arch and .driver == "llvmpipe" and .threads == "2" and
        (.vulkan_driver | contains($arch)) and .argv == ["Hello 日本語 🐈","$name; literal","a\"b\\c"]' "$scratch/managed-$platform.json" >/dev/null
done
started="$(w process start --timeout 10 -- "$guest\managed\anycpu\probe.exe")"
wait_process "$(jq -er .id <<<"$started")" ordinary-env
jq -e '.output | fromjson | .driver == null and .threads == null and .vulkan_driver == null' "$scratch/ordinary-env.json" >/dev/null
if w graphics prepare workspace/../outside.exe >/dev/null 2>&1; then echo 'graphics traversal accepted' >&2; exit 1; fi
printf 'existing application DLL\n' >"$scratch/collision.dll"
w export "$remote/managed/anycpu/probe.exe" "$scratch/managed.exe" >/dev/null
w import "$scratch/managed.exe" "$remote/collision/probe.exe" >/dev/null
w import "$scratch/collision.dll" "$remote/collision/opengl32.dll" >/dev/null
if w graphics prepare "$remote/collision/probe.exe" >/dev/null 2>&1; then echo 'foreign application DLL replaced' >&2; exit 1; fi
w export "$remote/collision/opengl32.dll" "$scratch/collision-result.dll" >/dev/null
cmp "$scratch/collision.dll" "$scratch/collision-result.dll"
if w export "$remote/collision/libgallium_wgl.dll" "$scratch/partial-link.dll" >/dev/null 2>&1; then echo 'partial deployment after preflight failure' >&2; exit 1; fi
if [ -z "${WDESK_TEST_SESSION:-}" ]; then w delete >/dev/null; fi
trap - EXIT
echo "Windows graphics passed ($engine); artifacts retained in $scratch"
