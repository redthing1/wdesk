---
name: wdesk
description: Inspect and control a Windows guest assigned through a wdesk session or descriptor, using screenshots, input, guest processes and files. Not for the host desktop or unrelated VMs.
---

# Windows desktop

Use the assigned `WDESK_DESCRIPTOR` or session; the trusted outer owner handles
open/stop/reset/delete and image preparation. Start with `wdesk --json
capabilities` and `wdesk --json see --output desktop.png`. Inspect the original
PNG. Coordinates refer to native pixels and never a resized preview.

Use `wdesk click X Y --generation N`, `wdesk type TEXT --generation N`, and
`wdesk key CTRL+S --generation N` using the latest observation's input generation.
Observe again after actions; delivery does not prove application success.
If input or epoch is stale, observe before issuing a new action. Reuse a stable
batch request id only with the identical body and within the runtime's cache.

`windows`, `a11y` and `clipboard get` supplement pixels. Secure/elevated desktop
input may require physical QEMU key/click commands; ordinary text uses Windows
SendInput at the helper's integrity. Use `type --console` for bounded US-layout
ASCII in consoles or before the helper is ready. A window id expires when its
window closes or its helper incarnation changes; re-inspect after a reboot.
Do not treat readiness as proof that a target application is responsive.

Guest paths use `workspace/` and `downloads/`. Import installers before running
them. `launch -- PROGRAM ARG...` and `process start -- PROGRAM ARG...` execute
only in Windows. Name `powershell.exe` or `cmd.exe` explicitly when a shell is
needed. `process wait`, `process output`, and `process kill` address owned ids.
Export useful artifacts before the trusted owner resets the disposable disk.
Owned processes retain bounded output and have a deadline; check `exit_code`,
`timed_out` and `output_truncated`. UIA is bounded and can fail on a hung provider;
use pixels when structured context is unavailable. Do not blindly repeat an
uncertain mutation with a new request id.

`wdesk view` gives a human the same console. Human input changes the generation.
No remote desktop login, engine socket, QMP socket, host mount, or lifecycle
credentials are needed by the agent.
