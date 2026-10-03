# wdesk

An agent-friendly Windows guest toolkit for Linux/KVM. One Rust CLI provides
native screenshots, input, guest processes and files, desktop context, and a
shared browser view. Optional Docker/Podman packaging uses the same QEMU runtime.

Install once, seal a base, and boot disposable overlays. The guest helper uses
inbox PowerShell and .NET; no guest Python, browser driver, or network service.

## Start

Requires x86-64 Linux, accessible `/dev/kvm`, Rust 1.90+, QEMU with PNG capture,
`xorriso`, and `mtools`. From this checkout, on Debian/Ubuntu:

```sh
sudo apt-get install qemu-system-x86 qemu-utils xorriso mtools
cargo install --locked --path .
wdesk doctor
wdesk image install --iso /path/to/windows.iso --name windows-lite --compress
wdesk open
wdesk view --browser
```

Windows 11 Pro, Tiny11, and Tiny11 Core are supported through optional
`reference`, `lite`, and `core` profiles. `wdesk image media` lists pinned Tiny11
presets; downloads are explicit and SHA-256 verified. See
[images and footprint](doc/images.md) for media, compression, and setup details.

Defaults: two CPUs, 4 GiB RAM, sparse 64 GiB disk. Both pinned 25H2 variants pass
acceptance at two CPUs and 2 GiB RAM on native KVM, Docker, and rootless Podman.
Your applications may need more.

## Use

```sh
wdesk see --output desktop.png
wdesk click 612 438
wdesk type 'Hello — 日本語'
wdesk key CTRL+S
wdesk windows
wdesk a11y
wdesk clipboard get
wdesk launch -- notepad.exe
wdesk import ./app.exe workspace/app.exe
wdesk process start --timeout 60 -- 'C:\ProgramData\wdesk\workspace\app.exe'
wdesk process wait PROCESS_ID --timeout 30
wdesk export downloads/result.json ./result.json
```

Use `--json` for machine output. Coordinates are native pixels; actions report
delivery, not application success. `click`, `type`, and `key` accept
`--generation N` to reject stale input. Guest execution stays in Windows;
remote files are scoped to `workspace/` and `downloads/`. See
[protocol](doc/protocol.md) and [agent instructions](skills/wdesk/SKILL.md).

## Manage

```sh
wdesk --session scratch open --image windows-lite --offline
wdesk --session scratch stop
wdesk --session scratch open
wdesk --session scratch reset
wdesk --session scratch delete
wdesk image build --engine podman
wdesk --session boxed open --image windows-lite --engine podman
```

`open` resumes recorded settings. `stop` keeps writes; `reset` starts a fresh
overlay. Export results first. Reset retains its old disk, and deletion moves
files to recovery trash; remove those copies separately to reclaim space.

State lives under `$XDG_DATA_HOME/wdesk` or `~/.local/share/wdesk`, overridable
with `WDESK_HOME`. Keep it private and outside Git. Never move a base with
dependent instances. The OCI runner packages Linux tooling, not native Windows
containers. Use `--engine docker` for Docker and `image build --source PATH`
when outside the checkout.

## Share

```sh
wdesk connect ./windows-client.json
WDESK_DESCRIPTOR=./windows-client.json wdesk --json capabilities
```

Descriptors grant desktop access without lifecycle or viewer credentials.
Keep them private. `view` provides a separate browser credential; browser and
agent input share one queue. APIs bind to loopback; raw VM control ports are not
published. Offline mode retains private helper control.

## Develop

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash tests/repository-check.sh
```

See [architecture](doc/architecture.md) and [testing](doc/testing.md).
Private notes and generated artifacts are excluded from Git and packaging.
