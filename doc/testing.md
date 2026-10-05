# Testing

## Source

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash tests/repository-check.sh
cargo build --locked
```

CI runs Rust, syntax, and repository checks—not Windows acceptance.
Repository checks also require Git, ripgrep, and Node.js; no npm packages.

## Runtime

Requires QEMU/KVM, `jq`, and `curl`. Windows tests need a prepared image.
Build the runner for each OCI engine with `wdesk image build --engine ENGINE`
from the checkout, or pass `--source PATH`.

```sh
bash tests/qemu-smoke.sh
WDESK_TEST_IMAGE=windows-lite bash tests/windows-e2e.sh native
WDESK_TEST_IMAGE=windows-lite bash tests/windows-files.sh native
WDESK_TEST_IMAGE=windows-lite bash tests/windows-processes.sh native
WDESK_TEST_IMAGE=windows-lite bash tests/windows-transfer-recovery.sh native
WDESK_TEST_IMAGE=windows-lite bash tests/windows-e2e.sh podman
WDESK_TEST_IMAGE=windows-lite bash tests/windows-e2e.sh docker
WDESK_TEST_IMAGE=windows-lite bash tests/isolation.sh podman
```

QEMU smoke checks capture, auth, retries, stale input, reset, and unlinked-binary
startup, storage accounting and recovery cleanup. Windows acceptance adds real
helper transport, Unicode input/clipboard,
files, bounded processes, x86 compatibility, focus, UIA, reboot, and reset.
Isolation checks two guests' files, clipboard, and credentials.
Linux mocks do not prove Windows behavior.

`tests/windows-ports.sh ENGINE` checks real TCP responses, multiple mappings,
offline routing, occupied ports, owner authority, stop/start/reset and revocation.
It uses a disposable guest, approves a fixture's UAC prompt, and creates only
narrow test-port firewall rules, removed when the fixture exits.

Process tests need a current helper and check x64/x86 completion
eviction, forget, deadlines, descendant cleanup and independent timer expiry
(advancing private timestamps rather than waiting ten minutes).
They also check control-reader data/EOF races, split UTF-8 and buffered lines.
File tests require a streaming-capable helper and cover interrupted upload/download,
explicit resume, source mismatch, idempotent receipts, cancellation, checksum
failure, locked destinations, Unicode/empty files, and desktop access during transfer.
Active recovery tests require correlated ranges and disconnect the guest during
each CLI stream. Each 61 MiB operation must resume and finish within 20 seconds;
override `WDESK_TEST_RECOVERY_TIMEOUT` for slower hosts.

Measure a 61 MiB import/export round trip with byte-for-byte verification:

```sh
WDESK_TEST_IMAGE=windows-lite bash tests/transfer-bench.sh native
```

The benchmark uses a new offline session at two CPUs/2 GiB and retains timings,
errors, and payloads. Override `WDESK_TEST_TRANSFER_MIB` or the per-direction
`WDESK_TEST_TRANSFER_TIMEOUT` (seconds, default 300); select an OCI engine as above.
It measures the helper's available transfer path; use `windows-files.sh` for
recovery checks. Set `WDESK_BIN=target/release/wdesk` for optimized timings.

Tests use new sessions, stop VMs on exit, and retain diagnostics. Deleted sessions
still occupy recovery trash. Defaults: `target/debug/wdesk`, image `windows-lite`,
two CPUs, 4 GiB RAM. Override `WDESK_BIN`, `WDESK_TEST_IMAGE`, `WDESK_TEST_MEMORY`
(MiB), or `WDESK_TEST_CPUS`. `WDESK_TEST_VERSION` checks Windows version;
`WDESK_TEST_COMPACT=1` requires CompactOS.

## Graphics

Requires a current graphics-capable helper, host `7z`, HTTPS access and both
MinGW-w64 C++ cross-compilers. No SDK is installed in Windows.

```sh
WDESK_TEST_IMAGE=windows-lite bash tests/windows-graphics.sh native
```

This checks x64/x86 OpenGL, GLES/EGL, Vulkan and Direct3D9/10/11/12 CPU rendering,
exact pixel readback, window presentation and clean exit. It also checks cache
reuse, hard links, managed bitness, literal argv, environment isolation and
deployment collision safety. Select `docker` or `podman` as above.
`WDESK_GXX64`/`WDESK_GXX86` override compiler paths; `WDESK_TEST_SESSION` tests an
existing disposable session without stopping, resetting or deleting it.

## Shares

With a current host-share-capable helper and the optional Samba backend:

```sh
WDESK_TEST_IMAGE=windows-lite bash tests/windows-shares.sh native
```

Select `docker` or `podman` after building the runner with `--shares`. This checks
live reads, read-only enforcement, Unicode writes, locking, symlink boundaries,
agent authority, offline isolation, reset reattachment and host-file retention.
For elevated acceptance, run `tests/share-probe.ps1 -Elevated` in the guest with
normal UAC approval; it also checks the negotiated SMB dialect and signing.

## Browser

Optional Playwright/Chromium dependencies stay on the host, outside the runner:

```sh
npm install --prefix /path/to/browser-tests playwright
/path/to/browser-tests/node_modules/.bin/playwright install chromium
NODE_PATH=/path/to/browser-tests/node_modules node tests/viewer-smoke.cjs
```

Chromium needs its host system libraries. Use an open default session or set
`WDESK_SESSION`. The test checks native geometry and Unicode readback through
the shared input queue using an inbox WinForms fixture.
