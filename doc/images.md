# Images

## Selection

`reference` keeps bundled apps; `lite` removes an allowlist of consumer apps.
`core` identifies reduced media, not additional wdesk pruning. All profiles
disable sleep and hibernation. wdesk does not disable UAC or Defender.

The optional presets pin October 2025 x86-64 artifacts and published SHA-256
hashes from the [Tiny11 25H2 listing](https://archive.org/details/tiny11_25H2).
Regular Tiny11 defaults to `lite`; Core defaults to `core`.
`--profile` overrides this; local ISOs default to `lite`.

```sh
wdesk image media
wdesk image fetch --media tiny11-25h2
wdesk image install --media tiny11-25h2 --name tiny11 --compress
wdesk image install --media tiny11-core-25h2 --name tiny11-core --compress
wdesk --session scratch open --image tiny11-core --memory 2048 --cpus 2 --offline
```

Downloads are explicit, hash-verified, and cached privately. Local `--iso`
never downloads. These are third-party customizations, not official Microsoft
media; a hash identifies an artifact, not its safety. See the
[upstream builder](https://github.com/ntdevlabs/tiny11builder) for regular/Core
differences. Core removes servicing/recovery components and inherits its media's
security defaults. Choose ordinary media when your tests need those components.
Do not delete WOW64, .NET, UI Automation, fonts, or system libraries indiscriminately.

## Footprint

Reference Windows 11 Pro 25H2 (26200.6584) measurements, in GiB:

| Image | ISO | Compressed host base | Guest C: used |
| --- | ---: | ---: | ---: |
| Tiny11, lite | 5.14 | 6.35 | 13.69 |
| Tiny11 Core | 2.96 | 3.70 | 10.46 |
| Core with CompactOS | 2.96 | 4.33 | 6.52 |

The CLI is about 6.5 MiB. At 2 GiB configured RAM, QEMU used approximately
2.1 GiB resident memory, plus 12–20 MiB for the Rust runtime. Initial overlays
were about 60 MiB, growing to about 130 MiB during probes. These are observations,
not workload limits; Windows directory totals also double-count hardlinks.

Sparse disk capacity is not allocated storage. Budget separately for the ISO,
installation disk, sealed base, overlays, and retained recovery copies. Sealing
temporarily needs both the installation disk and new base.

`--compress` on `image install`, `import`, or `seal` compresses the immutable host
base with Zstandard; overlays stay writable qcow2. Requires QEMU 5.1+ with
Zstandard support. Core-only `image install --compact-os` instead compresses
guest OS files without deleting them. Savings are not additive: CompactOS can
shrink guest usage while enlarging the compressed host base. Prefer ordinary
Core with `--compress` for smallest measured host storage.
[CompactOS reference](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/compact-os).

## Installation

`--index` selects the WIM image; `--install-key` selects the edition. The default
key selects Pro without activation. Windows licensing is separate.
`--disk-gb` accepts 24–512 GiB, defaulting to sparse 64 GiB; not every ISO fits 24 GiB.

Setup uses BIOS/MBR and hardware-check overrides, not TPM/Secure Boot. It wipes
only its new virtual disk and blocks external guest routing. Bases are private,
non-Sysprep snapshots; clones retain machine identity and the generated account password.

For asynchronous preparation:

```sh
wdesk image install --iso /path/to/windows.iso --name windows-lite --no-wait
wdesk --session build-windows-lite wait --timeout 1800
wdesk --session build-windows-lite image seal --name windows-lite --profile lite --compress
```

A timeout keeps the builder running: inspect `status`, `view`, and private logs;
resume with `open`/`wait`. After sealing, delete the stopped builder when no
longer needed, then remove its recovery trash to reclaim space. Sealed names
cannot be overwritten; dependent instances require their bases to remain in place.
