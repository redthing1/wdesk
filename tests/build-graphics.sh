#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
output="${1:?Pass a fixture output directory}"
mkdir -p "$output/headers/EGL" "$output/headers/GLES3" "$output/headers/KHR"
output="$(realpath "$output")"
fetch() {
    local url="$1" sha="$2" path="$3"
    curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
        --connect-timeout 10 --max-time 90 --max-filesize 33554432 "$url" -o "$path"
    printf '%s  %s\n' "$sha" "$path" | sha256sum --check --status
}
# Header-only test dependencies; the Windows guest receives no SDK or compiler.
egl_source=https://raw.githubusercontent.com/KhronosGroup/EGL-Registry/db3425b8246136faccb5e2782b5694960bd6edf1/api
gles_source=https://raw.githubusercontent.com/KhronosGroup/OpenGL-Registry/1cdd228e34966dd6b95bd203e9f84faba0f371a1/api
fetch "$egl_source/EGL/egl.h" a7c24c828bf2e1a4dfe6511b4f4812b948a285eb54787074015896976d4da076 "$output/headers/EGL/egl.h"
fetch "$egl_source/EGL/eglplatform.h" 25b5391655effcf363a38fa00c3154f356a3eab3d35dc067847875513cf1e441 "$output/headers/EGL/eglplatform.h"
fetch "$egl_source/KHR/khrplatform.h" 7b1e01aaa7ad8f6fc34b5c7bdf79ebf5189bb09e2c4d2e79fc5d350623d11e83 "$output/headers/KHR/khrplatform.h"
fetch "$gles_source/GLES3/gl3.h" a0e4880142bd059bd4d7446f257920b5020b8bbb0a86eb0149cbd1ea2fcf8cb0 "$output/headers/GLES3/gl3.h"
fetch "$gles_source/GLES3/gl3platform.h" a9e060dae5a2b11c5a889b679692b7089a10a7e03ebfbb6cf28217f6e322fb08 "$output/headers/GLES3/gl3platform.h"
fetch https://codeload.github.com/KhronosGroup/Vulkan-Headers/tar.gz/refs/tags/v1.4.363 \
    cbaf687d3c59b9666fe080e7f8396b6b4f4d344a768ee6b337c59852f4526b68 "$output/vulkan-headers.tar.gz"
tar -xzf "$output/vulkan-headers.tar.gz" -C "$output" Vulkan-Headers-1.4.363/include
flags=(-std=c++17 -O2 -Wall -Wextra -Werror -static)
for arch in x64 x86; do
    if [ "$arch" = x64 ]; then compiler="${WDESK_GXX64:-x86_64-w64-mingw32-g++}";
    else compiler="${WDESK_GXX86:-i686-w64-mingw32-g++}"; fi
    "$compiler" "${flags[@]}" tests/graphics-probe.cpp -lopengl32 -lgdi32 -luser32 \
        -ld3d9 -ld3d10 -ld3d11 -ld3d12 -ldxgi -o "$output/graphics-probe-$arch.exe"
    "$compiler" "${flags[@]}" -I "$output/headers" tests/graphics-egl.cpp -lgdi32 -luser32 -o "$output/graphics-egl-$arch.exe"
    "$compiler" "${flags[@]}" -I "$output/Vulkan-Headers-1.4.363/include" \
        tests/graphics-vulkan.cpp -luser32 -o "$output/graphics-vulkan-$arch.exe"
done
echo "Graphics fixtures built in $output"
