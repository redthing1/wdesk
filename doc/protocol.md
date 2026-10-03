# Protocol 1

Bearer-authenticated HTTP. All API routes are under `/v1/`. The API has no VM
lifecycle methods. `capabilities` exposes the epoch, guest readiness and limits.
`health` exposes readiness, geometry and input generation without credentials.

`GET /v1/see` returns a lossless native PNG. The `x-wdesk-observation` header
contains JSON with runtime epoch, helper incarnation (`guest_epoch`, null when
unavailable), observation id, input generation, dimensions,
SHA-256, byte length, coordinate space and cursor inclusion (currently false).

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
`type_ascii` (`text`, at most 1024 ASCII characters using the US keyboard
layout), `wait` (`milliseconds`). Maximum 64 actions and 10 seconds of waits.
Total typing per batch is bounded to 16 KiB UTF-8 Unicode and 1024 ASCII bytes. All
actions are validated before execution against the last captured geometry.
Unknown fields are rejected. `expected_input_generation: null` omits the stale
input check. Epoch validation always applies. An input-generation check does
not detect UI changes caused by applications, timers or network responses.
Helper restarts advance input generation. Window ids are valid only for their
helper incarnation; inspect again after a guest reboot or helper restart.

Results include before/after generations, per-action `delivered`, count of
completed actions, and `application_success: null`. Execution stops on first
error; delivery can be uncertain if a transport or native operation fails.
The last 256 request ids are retained. Identical retries return the recorded
result; changed content under the same id is rejected. Do not retry uncertain
input after eviction or restart without observing again.

`POST /v1/guest` accepts `{ "op": "windows", "args": {} }`. Supported methods
cover health, windows/focus, clipboard get/set, Unicode text, launch, process
start/status/kill, bounded UIA, and file begin/write/commit/abort/stat/read.
The helper validates operation-specific fields. File transfers are at most
4 GiB with 48 KiB chunks, ordered offsets and SHA-256 integrity. Owned process
deadlines are 1..3600 seconds. Explicitly launched GUI applications are not
owned processes and live until Windows shutdown or application exit.
Owned output retains at most 65,536 characters while continuing to drain the
pipe. At most 128 owned process ids are retained until helper restart.
Captured process output is decoded as UTF-8 (with BOM detection); programs that
emit a legacy OEM code page should be configured to emit UTF-8 explicitly.
Accessibility nodes use `bounds: null` when a provider has no finite rectangle;
do not interpret missing bounds as a zero-sized rectangle at the origin.

Viewer credentials can call only health, see and batch. They cannot inspect
files, start processes, read clipboard or obtain window/accessibility data.
Text entry in a viewer batch still uses the private helper, preserving the
common input queue. No raw VM input endpoint bypasses that queue.

Operational errors use JSON `{ "ok": false, "error": { "code", "message",
"retryable" } }`; malformed JSON/schema errors are HTTP 4xx extractor responses.
Authentication failure is 401, stale epoch/input and request-id conflict are
409, and unavailable QMP/helper operations are 503.
