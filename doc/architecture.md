# Architecture

One host CLI manages one runtime per guest. Native mode and OCI packaging use
the same Rust runtime and QEMU/KVM; there is no fleet daemon.

```text
trusted host CLI ─── native process or Podman/Docker owner
                         │
agent CLI ── authenticated API ── per-VM Rust runtime ── QEMU/KVM
browser ── viewer credential ────┤                         │
                                ├─ private QMP: pixels/input
                                └─ private guestfwd/serial: Windows helper
                                                        ├─ Win32 windows/input
                                                        ├─ Job Object processes
                                                        ├─ scoped NTFS files
                                                        └─ bounded UIA worker
```

## Runtime

The builder creates a seed CD and FAT answer-file disk. IDE storage, VGA, USB
tablet, and COM1 use inbox drivers. QMP and helper sockets stay private to the
runtime. The helper connects outward through one guestfwd rule; offline mode
blocks external routing without losing control. Serial is the recovery path.
No inbound guest ports are forwarded.

Readiness progresses through `vm_running`, `guest_responding`, `helper_ready`,
and `automation_ready` after native capture. Elevated first-logon provisioning
reboots before the helper starts in the interactive console. Its administrator
account normally runs at user integrity with UAC; secure-desktop input uses QEMU.

## State and input

Sealing flattens a session into an immutable standalone base with media/helper
hashes and guest inventory. Experiments share its disk blocks through independent
overlays, not RAM. Every runtime boot rotates data/viewer credentials and the
epoch. Reset cold-boots a new overlay without reusing saved RAM or firmware state.

Helper incarnations qualify window ids and advance input generation, including
across temporary health loss. One mutex serializes captures and whole
browser/agent batches. Attempted input advances generation even on uncertain
delivery; releases run on failure. Identical retained retries return the original
result; changed bodies are rejected. See [protocol](protocol.md).

## Guest boundary

One STA helper loop dispatches requests; ids distinguish late replies.
Owned processes use Windows argv quoting, suspended creation, Job Object assignment,
then resume. Output keeps draining after its retention bound. UIA uses an owned
worker with an eight-second timeout. Chunked files verify SHA-256 before committing.

Guest execution never runs on Linux. File scoping rejects traversal, device
names, drive syntax, and reparse components, and verifies opened handles against
allowed NTFS roots. It is a convenience boundary, not isolation from a malicious
process already controlling the guest. Lifecycle/autologon credentials stay out
of agent descriptors.

The BIOS/MBR profile uses hardware-check overrides, not TPM/Secure Boot.
Non-Sysprep bases retain machine identity. No live-memory snapshots or
high-frame-rate streaming. wdesk does not disable UAC or Defender; third-party
media may differ. See [images](images.md) for preparation and footprint.
