# Protocol 1

Bearer-authenticated HTTP under `/v1/`. No VM lifecycle methods.
`GET /v1/capabilities` reports epoch, readiness, and limits;
`GET /v1/health` reports readiness, geometry, and input generation.
Responses omit credentials.

## Observations

`GET /v1/see` returns a lossless native PNG. The `x-wdesk-observation` header
contains JSON with runtime epoch, helper incarnation (`guest_epoch`, nullable),
observation id, input generation, dimensions, SHA-256, byte length, coordinate
space, and cursor inclusion (currently false). Coordinates are native pixels.

## Input

`POST /v1/batch` accepts strict JSON:

```json
{
  "protocol": 1,
  "request_id": "save-1",
  "epoch": "EPOCH_FROM_OBSERVATION",
  "expected_input_generation": 4,
  "actions": [
    { "type": "click", "x": 400, "y": 300, "button": "left" },
    { "type": "type_text", "text": "hello" },
    { "type": "key", "keys": ["CTRL", "S"] }
  ]
}
```

Other actions: `move`, `drag` (`x/y/to_x/to_y`), `scroll` (`direction/steps`),
`type_ascii` (`text`, US layout), and `wait` (`milliseconds`). Per batch: at most
64 actions, 10 seconds of waits, 16 KiB UTF-8 Unicode, and 1024 ASCII bytes.
All actions are validated against the last captured geometry before execution.
Unknown fields are rejected. Epoch validation is mandatory;
`expected_input_generation: null` skips only the generation check.

Generation checks do not detect application/timer/network changes. Helper
restarts advance generation; window ids expire with their helper incarnation.
Observe again after reboot or stale input.

Results report before/after generations, per-action `delivered`, completed
action count, and `application_success: null`. Execution stops at the first
error; delivery may be uncertain. The last 256 request ids are retained.
Identical retries return the recorded result; changed bodies are rejected.
Do not replay uncertain input after eviction or restart without observing again.

## Guest operations

`POST /v1/guest` accepts `{ "op": "windows", "args": {} }`. Methods cover health,
windows/focus, clipboard get/set, Unicode text, launch, process start/status/kill,
bounded UIA, and file begin/write/commit/abort/stat/read. Fields are validated
per operation.

Transfers are at most 4 GiB, using ordered 48 KiB chunks and SHA-256 verification.
Owned processes have 1–3600-second deadlines and retain 65,536 output characters
while continuing to drain pipes. Up to 128 owned records are retained and cleared
on helper restart.
Output is decoded as UTF-8 with BOM detection; configure legacy-code-page programs
to emit UTF-8. `launch` processes are unowned and live until exit or shutdown.
UIA uses `bounds: null` when a provider has no finite rectangle, not an origin rectangle.

Viewer credentials allow only health, see, and batch—not files, processes,
clipboard, windows, or UIA. Viewer text input still uses the helper through
the common queue; no raw input endpoint bypasses it.

## Errors

Operational errors use `{ "ok": false, "error": { "code", "message", "retryable" } }`.
Malformed JSON/schema errors are HTTP 4xx extractor responses. Authentication
failure is 401; stale epoch/generation and request-id conflicts are 409;
unavailable QMP/helper operations are 503.
