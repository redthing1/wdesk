# wdesk

An agent-friendly Windows guest toolkit for Linux/KVM. One Rust CLI provides
native screenshots, input, Windows processes and files, desktop context, and a
shared browser view. Optional Docker/Podman packaging uses the same QEMU runtime.

Prepare once. Seal a known-good environment. Run experiments in independent
copy-on-write disks, then reset them to the baseline. No guest Python, browser
driver, or network service; the helper uses inbox PowerShell and .NET.

## Start

Requires x86-64 Linux, accessible `/dev/kvm`, Rust 1.90+, QEMU with PNG capture,
`xorriso`, and `mtools`. From this checkout, on Debian/Ubuntu:

```sh
sudo apt-get install qemu-system-x86 qemu-utils xorriso mtools
cargo install --locked --path .
wdesk doctor
wdesk image install --media tiny11-25h2 --name windows-lite --compress
wdesk open
wdesk view --browser
```

This example explicitly downloads a pinned, SHA-256-verified Tiny11 preset.
For your own Windows 11 Pro media, replace `--media tiny11-25h2` with
`--iso /path/to/windows.iso`. See [images](doc/images.md) for profiles,
third-party media caveats, and footprint.

Defaults: two CPUs, 4 GiB RAM, sparse 64 GiB disk. Regular Tiny11 and Core 25H2
pass acceptance at two CPUs and 2 GiB RAM on native KVM, Docker, and rootless
Podman. Applications may need more.

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

Use `--json` for machine output; import/export progress goes to stderr.
New helpers stream files directly, without mounts. Interrupted uploads can use
`import --resume TRANSFER_ID`; `transfer status` and `transfer cancel` inspect or
release them. Resume requires the same source, destination, and helper incarnation.
Coordinates are native pixels; delivery does not prove application success.
`click`, `type`, and `key` accept `--generation N` to reject stale input.
Guest execution stays in Windows; files are scoped to
`workspace/` and `downloads/`. See [agent instructions](skills/wdesk/SKILL.md)
and [protocol](doc/protocol.md).

Optional [software graphics](doc/graphics.md) supports CPU rendering without a
physical GPU. Install only the required APIs and application architecture;
the core guest remains free of graphics SDKs and toolchains.

## Experiment

[Prepare a baseline](doc/images.md#prepared-environments) with your tools, then
use a new session name for each experiment:

```sh
wdesk --session exp-a open --image lab-v1 --memory 2048 --cpus 2 --offline
wdesk --session exp-a stop
wdesk --session exp-a open
wdesk --session exp-a reset
wdesk --session exp-a delete
```

`open` resumes recorded settings. `stop` keeps writes; `reset` cold-boots the
baseline. Export results first. Reset retains its old disk; deletion moves files
to recovery trash. `wdesk storage` reports file sizes, allocated space, and exact
cleanup targets. `wdesk prune TARGET` previews removal; add `--execute` to
permanently remove that recovery copy. Bases and current disks are never targets.

State defaults to `$XDG_DATA_HOME/wdesk` or `~/.local/share/wdesk`; override with
`WDESK_HOME`. Keep it private and outside Git. Do not move or remove a base
while dependent sessions exist.

## Containers and sharing

```sh
wdesk image build --engine podman
wdesk --session boxed open --image lab-v1 --engine podman
wdesk --session boxed connect ./windows-client.json
WDESK_DESCRIPTOR=./windows-client.json wdesk --json capabilities
```

Use `--engine docker` for Docker and `image build --source PATH` outside the
checkout. OCI packages Linux tooling, not native Windows containers. The API
is published on loopback; raw VM control ports are not exposed. Offline mode
retains private helper control.

Keep descriptors private: they grant desktop access, not lifecycle or viewer
credentials. `view` supplies a separate browser credential. Browser and agent
input share one queue.

Optional [live shares](doc/shares.md) grant specific host directories; read-only
is the default. Direct transfers remain independent and need no share service.

## Develop

See [testing](doc/testing.md) for checks and [architecture](doc/architecture.md)
for internals. Private notes and generated artifacts stay out of Git and packaging.
