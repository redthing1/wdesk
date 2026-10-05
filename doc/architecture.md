# Architecture

One host CLI manages one runtime per guest. Native mode and OCI packaging use
the same Rust runtime and QEMU/KVM; there is no fleet daemon.

```text
trusted host CLI ─── native process or Podman/Docker owner
                         │
agent CLI ── authenticated API ── per-VM Rust runtime ── QEMU/KVM
browser ── viewer credential ────┤                         │
                                ├─ private QMP: pixels/input
                                ├─ private guestfwd/serial: console helper
                                └─ private guestfwd: binary file worker
```

## Runtime

The builder creates a seed CD and FAT answer-file disk. IDE storage, VGA, USB
tablet, and COM1 use inbox drivers. QMP and helper sockets stay private to the
runtime. The helper connects outward through private guestfwd channels; offline mode
blocks external routing without losing control. Serial is the recovery path.
No inbound guest ports are forwarded by default. Owner-configured TCP forwards
target the VM's DHCP address, never private helper endpoints. Native QEMU binds
loopback; OCI normalizes bridge peers through bounded in-runtime TCP relays and
loopback-only engine publications, preserving offline guests' return paths.
See [networking](networking.md).

Optional live shares use a per-VM unprivileged Samba child. A private Unix socket
and per-connection guestfwd relay preserve SMB connection lifetimes; no host SMB
port is published by OCI. Only owner configuration grants host directories.
The runtime attaches them through a private helper operation excluded from the
agent protocol. See [shares](shares.md).

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
worker with an eight-second timeout. Process handles retire independently; at most
128 completion receipts survive for ten minutes. Closing an owned parent also
ends its job's descendants. A separate bounded file worker streams binary
ranges and hashes off the STA thread. Uploads verify SHA-256, then atomically
rename within NTFS using a verified parent handle. Transfers never require a mount.

Guest execution never runs on Linux. File scoping rejects traversal, device
names, drive syntax, and reparse components, and verifies opened handles against
allowed NTFS roots. It is a convenience boundary, not isolation from a malicious
process already controlling the guest. Lifecycle/autologon credentials stay out
of agent descriptors.

The BIOS/MBR profile uses hardware-check overrides, not TPM/Secure Boot.
Non-Sysprep bases retain machine identity. No live-memory snapshots or
high-frame-rate streaming. wdesk does not disable UAC or Defender; third-party
media may differ. See [images](images.md) for preparation and footprint.
