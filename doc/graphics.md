# Software graphics

No physical GPU is required. The optional `software-v1` preset supplies
application-local Mesa llvmpipe/lavapipe for OpenGL, GLES/EGL and Vulkan.
Windows supplies Direct3D software rendering; applications must select WARP
where their Direct3D backend requires it. This is not a universal GPU substitute.

```sh
wdesk graphics install --arch x64 --apis gl,gles,vk
wdesk import ./app.exe workspace/app/app.exe
wdesk graphics run workspace/app/app.exe --apis gl,gles,vk -- --app-option
wdesk process wait PROCESS_ID
```

Select only the APIs needed; the default is `gl`. Install `--arch x86` for
32-bit applications. `prepare` and `run` detect native and managed PE bitness.
`prepare` links DLLs without launching; `run` additionally supplies process-local
renderer settings, an application-directory working directory and an owned
process deadline. It does not change global environment variables or system DLLs.

Installation requires host HTTPS access and `7z`, but works with an offline
guest. Downloads are pinned by SHA-256; vendor installers are not executed.
The manifest records upstream license references and retains the Vulkan loader
notice. Runtime binaries are fetched on demand, not bundled in the repository.
Components are cached once per architecture and linked into clean application
directories using NTFS hard links. Existing application DLLs are never replaced.
Repeated installation verifies hashes and reuses matching components.

The full kits contain about 115 MiB (x64) or 98 MiB (x86) of runtime files;
x64 OpenGL alone is about 59 MiB. Host compressed downloads total about 85 MiB.
Hard links do not duplicate DLL contents. These are file sizes, not measured
qcow2 allocation. Small x64 pixel/presentation probes peaked at about 56 MiB
(OpenGL) and 42 MiB (Vulkan) process working set; real applications vary.
Shader compilation and rendering need application-dependent CPU and memory.
No guest SDK, compiler, driver service or graphics package is mandatory.

`graphics status` reports component presence, not application compatibility or
verified rendering. Use [graphics acceptance](testing.md) to test actual pixels,
presentation and clean process exit; check an application's renderer rather than
assuming that successful launch proves software rendering.
