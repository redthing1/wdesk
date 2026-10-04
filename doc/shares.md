# Live host shares

Transfers copy files into guest-local NTFS. Shares expose a live host directory;
neither synchronization nor VM rollback applies to those files.

The lifecycle owner grants existing directories while the VM is stopped:

```sh
wdesk --session lab stop
wdesk --session lab share add source /path/to/project
wdesk --session lab share add output /path/to/experiment-output --write
wdesk --session lab open
wdesk --session lab capabilities
```

The guest uses `\\10.0.2.102\source` and `\\10.0.2.102\output`. Read-only is
the default. Use `share list` to inspect owner configuration and `share remove
NAME` while stopped to revoke a grant. A replaced directory requires a fresh
grant. Up to eight names are supported; configuration metacharacters in host
paths are rejected. Private wdesk state cannot be shared.

Native mode requires optional `samba` and `libnss-wrapper` packages. For OCI,
build the runner with `wdesk image build --engine podman --shares` (or `docker`).
The default runner has no Samba dependency. Both modes use an unprivileged,
per-VM server and the Windows inbox SMB client. Authentication rotates each
runtime boot; signing is mandatory and symlinks are not followed. The guest
endpoint remains private, including offline mode; OCI publishes no SMB port.
Agent descriptors cannot grant directories or carry share credentials.

The optional OCI layer adds about 147 MiB. A native server and relay used about
20 MiB proportional resident memory in one sample; usage varies with activity.

`open` waits for attachment; `capabilities.shares.attached` reports its state.
A current helper is required. Use UNC paths rather than drive-letter mappings;
keep installed applications and filesystem-sensitive work on guest-local NTFS.

Host writes survive stop, reset and VM deletion. Use a separate writable output
directory for each experiment. Reset reattaches the recorded grants; deleting a
VM never deletes host directories. Remove grants before sealing an image.
