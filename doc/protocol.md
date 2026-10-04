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
windows/focus, clipboard get/set, Unicode text, launch, process start/status/kill/forget,
bounded UIA, and file begin/write/commit/abort/stat/read. Fields are validated
per operation.

Legacy file operations use ordered 48 KiB JSON chunks without resume. New helpers
advertise `file_transfer.binary_stream`; prepare a new image to enable it.
Owned processes have 1–3600-second deadlines and retain 65,536 output characters
while continuing to drain pipes. With `process_retention.bounded`, up to 128
active processes and 128 completion receipts are retained. Receipts expire after
ten minutes or helper restart; `process_forget` releases a completed receipt.
`phase` distinguishes running, draining and completed; `output_complete` reports
whether the redirected output reached EOF. Parent exit ends owned descendants.
Older helpers retain 128 records until restart.
Output is decoded as UTF-8 with BOM detection; configure legacy-code-page programs
to emit UTF-8. `launch` processes are unowned and live until exit or shutdown.
UIA uses `bounds: null` when a provider has no finite rectangle, not an origin rectangle.

## Files

`POST /v1/transfers` starts an upload/download with strict JSON fields
`epoch`, `helper_id`, `id` (canonical UUID), `direction`, `path`, `size`, and
`sha256`. Uploads declare source size/hash; downloads hold a stable read handle.
`GET /v1/transfers/{id}` takes `epoch` and `helper_id` query parameters.
Receipts report phase, accepted offset, active range, hash, and error.

`PUT`/`GET /v1/transfers/{id}/data` stream raw bytes with query parameters
`epoch`, `helper_id`, `offset`, and `length`. Upload offsets must match the
worker's accepted offset. `POST .../pause`, `.../commit`, and `.../abort` take
`{epoch, helper_id}`. Pause interrupts a range without discarding its stage;
reconcile status before reconnecting. Commit is idempotent and asynchronous.
When `file_transfer.range_identity` is true, a range accepts optional `request_id`
(canonical UUID); its receipt's `range_id` correlates interruptions with that
attempt. An old error does not describe a new range.

Files are limited to 4 GiB. Binary transfers retain eight active handles, two hash
workers, one streaming lane, and 256 receipts per helper. Buffers are 256 KiB; idle I/O
expires after 30 seconds. Transfers expire after ten inactive minutes or helper
restart, not across cold boots. Accepted bytes are not durable storage.
Uploads preserve the destination until verified atomic replacement. Downloads
verify the hash before local replacement. Neither operation needs a host mount.

Viewer credentials allow only health, see, and batch—not files, processes,
clipboard, windows, or UIA. Viewer text input still uses the helper through
the common queue; no raw input endpoint bypasses it.

## Errors

Operational errors use `{ "ok": false, "error": { "code", "message", "retryable", "outcome" } }`.
`outcome` is `not_started` or `unknown`. A failed guest mutation may already have
run; it is not marked retryable. Read-only transport failures can be retried;
explicit helper failures use `guest_rejected` and require inspecting the cause.
Malformed JSON/schema errors are HTTP 4xx extractor responses. Authentication
failure is 401; stale epoch/generation and request-id conflicts are 409;
unavailable QMP/helper operations are 503.
