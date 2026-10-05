# Networking

Guest applications can be reached through explicit TCP forwards. Nothing is
forwarded by default; host listeners are always IPv4 loopback, not LAN-facing.
Native, Docker, and Podman use the same interface, with no extra guest service.
After upgrading the CLI, rebuild the OCI runner before using forwarding.

```sh
wdesk --session web open --forward 8080:8080 --offline
# Start the service in Windows before connecting.
curl http://127.0.0.1:8080
wdesk --session web port list
wdesk --session web stop
wdesk --session web port add 9222:9222
wdesk --session web port remove 8080
wdesk --session web open
```

`--forward HOST_PORT:GUEST_PORT` is repeatable when creating a session. `open`
resumes saved mappings; use `port add/remove` while stopped to change them.
Mappings survive stop/start and reset; stopping or deleting closes listeners.
`port list` and local-owner `status` report configured host endpoints, not
application readiness. Agent descriptors cannot change forwarding.

Use distinct host ports for parallel sessions. Ports must be 1–65535, host ports
must be unique per session, and at most 16 mappings are allowed. Occupied host
ports fail startup. Automatic allocation, UDP, and live changes are not supported.

The Windows application must listen on its network interface or `0.0.0.0`, not
only guest localhost. Windows Firewall must allow that application/port; wdesk
does not disable it or add rules automatically. Forwarding adds no authentication
to the application: other local host processes can connect.

`--offline` blocks ordinary external routing while retaining explicitly granted
forwards and private helper/share channels. QMP, serial, and raw VNC stay private.
OCI publishes only loopback host ports; application listeners inside its network
namespace are reachable according to the engine's container-network policy.
Its in-runtime relays allow 64 concurrent connections per VM; excess connections
close. Native mode forwards directly through QEMU.
