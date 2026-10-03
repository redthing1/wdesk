# Architecture

One host CLI manages one runtime per guest, using native QEMU/KVM or the same
runtime in an OCI runner. No fleet daemon. The runtime owns the authenticated
API, viewer, and shared action queue.

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

The builder creates a seed CD and FAT answer-file disk on the host. IDE storage,
VGA, USB tablet, and COM1 use inbox drivers. QMP and helper sockets stay in a
private per-runtime directory. User-mode networking has no inbound guest port
forwarding. The helper connects outward through one guestfwd rule; offline mode
blocks external routing without losing control. Serial is the recovery path.

Readiness progresses through `vm_running`, `guest_responding`, `helper_ready`,
and `automation_ready` after native capture. The helper starts in the interactive
console after elevated first-logon provisioning reboots. Its administrator
account normally runs at user integrity with UAC; secure-desktop interactions
use physical QEMU input.

Each boot rotates data/viewer credentials and the runtime epoch. Reset creates
an independent cold-boot overlay, never reusing RAM or firmware state. Sealed
standalone bases record media/helper hashes and guest inventory. Helper
incarnations qualify window ids and advance input generation, including across
temporary health loss.

One mutex serializes captures and whole browser/agent batches. Attempted input
advances generation even on uncertain delivery; releases run on failure.
Retained identical retries return the original result. Changed request bodies
are rejected. See [protocol](protocol.md) for limits and retry semantics.

The helper uses one STA dispatch loop; request ids distinguish late replies.
Processes use Windows argv quoting and suspended creation, enter a Job Object,
then resume. Output continues draining after its retention bound. UIA runs in
an owned worker with an eight-second timeout so a hung provider cannot wedge
the helper. Chunked file transfers verify SHA-256 before committing.

Guest execution never runs on Linux. File scoping rejects traversal, device
names, drive syntax, and reparse components, and verifies opened handles against
allowed NTFS roots. This is a convenience boundary, not isolation from a
malicious process already controlling the guest. Lifecycle and autologon
credentials stay out of agent descriptors.

The BIOS/MBR profile uses hardware-check overrides, not TPM/Secure Boot.
Non-Sysprep bases retain machine identity. No live-memory snapshots or
high-frame-rate streaming. wdesk does not disable UAC or Defender; third-party
media may differ. See [images](images.md) for preparation and profile tradeoffs.
