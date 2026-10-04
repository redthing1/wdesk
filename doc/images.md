# Images and environments

## Choose media

`reference` keeps bundled apps; `lite` removes an allowlist of consumer apps.
`core` identifies reduced media, not additional wdesk pruning. All profiles
disable sleep and hibernation; wdesk does not disable UAC or Defender.

```sh
wdesk image media
wdesk image install --media tiny11-25h2 --name windows-lite --compress
wdesk image install --media tiny11-core-25h2 --name tiny11-core --compress
```

Presets pin October 2025 x86-64 artifacts and published SHA-256 hashes from the
[Tiny11 25H2 listing](https://archive.org/details/tiny11_25H2). Downloads are
explicit, verified, and cached privately. `image fetch --media ID` downloads
without installing; local `--iso PATH` never downloads. Presets default to
`lite` or `core`; local ISOs default to `lite`. Override with `--profile`.

These are third-party customizations, not official Microsoft media. A matching
hash identifies an artifact, not its safety. Core removes servicing/recovery
components and inherits its media's security defaults; choose ordinary media
when tests need those components. See the [upstream builder](https://github.com/ntdevlabs/tiny11builder).
Preserve the system libraries, WOW64, .NET, fonts, and UI Automation your tools need.

## Prepared environments

Starting from the quick-start image:

```sh
wdesk --session prepare open --image windows-lite
# Install tools, finish reboots, close applications, and verify the environment.
wdesk --session prepare image seal --name lab-v1 --profile lite --compress
wdesk --session exp-a open --image lab-v1 --memory 2048 --cpus 2 --offline
wdesk --session exp-b open --image lab-v1 --memory 2048 --cpus 2 --offline
wdesk --session exp-a reset
```

For Core, use `--image tiny11-core` and seal with `--profile core`.
Sealing stops the preparation VM and creates an immutable standalone base;
sealed names cannot be overwritten.
Each new session name gets an independent overlay sharing that base—not a full
disk copy or running-memory clone. Windows cold-boots; RAM is per running VM.

`reset` discards current state and boots the same baseline; `stop` preserves
changes without keeping the VM running. Export results before reset/delete.
To update the environment, prepare and seal `lab-v2`, then create new sessions
from it. Existing sessions retain their original image and configuration.

Delete the preparation session when no longer needed. Deleted sessions go to
private `trash/`; reset retains `system-before-reset-*.qcow2` in the session
directory. `wdesk storage` lists exact recovery targets; `wdesk prune TARGET`
previews removal and `--execute` permanently removes that copy. No automatic
garbage collection. Keep bases in place while sessions depend on them.

## Footprint

Reference Windows 11 Pro 25H2 (26200.6584) measurements, in GiB:

| Image | ISO | Compressed host base | Guest C: used |
| --- | ---: | ---: | ---: |
| Tiny11, lite | 5.14 | 6.35 | 13.69 |
| Tiny11 Core | 2.96 | 3.97 | 11.02 |
| Core with CompactOS | 2.96 | 4.33 | 6.52 |

The stripped CLI is about 8 MiB. At 2 GiB configured RAM, QEMU used roughly 2.1 GiB
resident memory, plus 8–20 MiB for the Rust runtime in native tests. Early
overlays used about 60 MiB, growing to about 130 MiB during probes. These are
observations, not workload limits. Snapshots vary with OS activity and installed tools;
Windows directory totals can double-count hardlinks.

Sparse capacity is not allocated storage. Budget for media, preparation disks,
sealed bases, overlays, and recovery copies. Sealing temporarily needs both the
source disk and new base; each sealed environment is a standalone image.

`--compress` on `image install`, `import`, or `seal` uses host-side Zstandard
(QEMU 5.1+ with Zstandard support); writable overlays remain ordinary qcow2.
Core-only `image install --compact-os` compresses guest OS files without removing
them. Savings are not additive: CompactOS can shrink guest usage but enlarge
the compressed host base. Ordinary Core with `--compress` had the smallest
measured host footprint. See [CompactOS](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/compact-os).

## Installation notes

`--index` selects the WIM image; `--install-key` selects the edition. The default
key selects Pro without activation. Windows licensing is separate.
`--disk-gb` accepts 24–512 GiB, defaulting to sparse 64 GiB; not every ISO fits 24 GiB.

Setup wipes only its new virtual disk and blocks external guest routing.
The [BIOS/MBR compatibility profile](architecture.md) is not TPM/Secure Boot.
Bases are private, non-Sysprep snapshots retaining machine identity and account password.

A timeout leaves `build-NAME` running for `status`, `view`, and log inspection.
Resume with `open`/`wait`. With `image install --no-wait`, finish using
`wdesk --session build-NAME wait --timeout 1800`, then
`wdesk --session build-NAME image seal --name NAME --profile PROFILE --compress`.
