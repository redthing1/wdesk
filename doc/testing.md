# Testing

## Source checks

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash tests/repository-check.sh
cargo build --locked
```

CI runs Rust, syntax, and repository checks, not Windows acceptance.
Repository checks also need Git, ripgrep, and Node.js; no npm packages.

## Runtime acceptance

Requires QEMU/KVM, `jq`, and `curl`; Windows tests need a prepared image.
Build the OCI runner before engine tests with `wdesk image build --engine ENGINE`.
Run from the checkout or pass `--source PATH`.

```sh
bash tests/qemu-smoke.sh
WDESK_TEST_IMAGE=windows-lite bash tests/windows-e2e.sh native
WDESK_TEST_IMAGE=windows-lite bash tests/windows-e2e.sh podman
WDESK_TEST_IMAGE=windows-lite bash tests/windows-e2e.sh docker
WDESK_TEST_IMAGE=windows-lite bash tests/isolation.sh podman
```

QEMU smoke covers capture, authentication, retries, stale input, reset, and
unlinked-executable startup. Windows acceptance adds real helper transport,
Unicode input/clipboard, files, bounded processes, x86 compatibility, focus,
UIA, reboot, and reset isolation. Isolation checks two guests' files, clipboard,
and credentials. Linux mocks do not prove Windows behavior.

Tests create new sessions, stop VMs on exit, and retain diagnostics. Successful
deletion retains disks in recovery trash. Override `WDESK_BIN`, `WDESK_TEST_IMAGE`,
`WDESK_TEST_MEMORY` (MiB), or `WDESK_TEST_CPUS`. Use `WDESK_TEST_VERSION` to check
the Windows version and `WDESK_TEST_COMPACT=1` to require CompactOS.

## Browser

Optional Playwright/Chromium dependencies stay on the host, outside the runner:

```sh
npm install --prefix /path/to/browser-tests playwright
/path/to/browser-tests/node_modules/.bin/playwright install chromium
NODE_PATH=/path/to/browser-tests/node_modules node tests/viewer-smoke.cjs
```

Chromium also needs its host system libraries. Use an open default session or
set `WDESK_SESSION`. The test checks native canvas geometry and Unicode readback
through the shared input queue using an inbox WinForms fixture.
