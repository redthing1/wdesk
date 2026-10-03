# Working on wdesk

wdesk is a Linux/KVM Windows guest toolkit. One Rust CLI owns optional OCI
packaging and speaks to one per-VM runtime. The Windows helper runs in the
interactive console session. Read README.md and doc/architecture.md.
For guest-control work, also read skills/wdesk/SKILL.md.

Keep notes/ private, internal, and untracked. Use it for detailed decisions,
local paths, measurements, and validation evidence. All other repository content,
including doc/, is concise public documentation: use portable examples and omit
user identities, machine-specific state, credentials, and private-note links.

Keep guest process/file execution in Windows. Never evaluate guest arguments
on the host. Screenshots retain native geometry. All browser and agent input
shares a serialized queue; stale observations and partial delivery are explicit.
Lifecycle credentials stay out of data-plane descriptors. Do not publish QMP,
serial control, or raw VNC ports. Do not disable UAC or Defender for convenience.
Sealed images are immutable; instances use independent cold-boot overlays.
Use explicit arguments for child processes, bounded output, and scoped file paths.

Verify with cargo fmt --check, cargo test, cargo clippy --all-targets -- -D warnings.
Runtime changes also require tests/qemu-smoke.sh and, when installation media is
available, tests/windows-e2e.sh. Record actual results and limitations in notes/.
Do not claim Windows behavior has been tested solely from Linux mocks.
