# pico_w_display

Firmware for the Plasma 2350 W LED matrix: renders a clock or client-set text/color, controlled over one or more transports.

## Language

**Transport**:
A channel a client can open a Session over — USB CDC-ACM serial or the BLE GATT link. `run_session` is written once against `embedded_io_async::Read`/`Write` and is identical regardless of which Transport carries it.
_Avoid_: Connection type, interface

**Session**:
One client's connected lifetime on a Transport: read a command line, dispatch it, wait for the display to ack, write back a reply. Ends when the Transport errors or closes; the caller then waits for a new one.
_Avoid_: Connection, request

**Command**:
A parsed instruction a Session dispatches to the display loop: `TEXT`, `CLOCK`, `COLOR`, `BRIGHTNESS`, `WIFI`, `UNPAIR`. Transport-agnostic — the display loop only ever sees a `Command`, never which Transport it arrived over.

**Bond**:
The persisted cryptographic pairing record (identity + LTK) that lets a previously-paired phone re-establish an encrypted BLE link with no user action. Exactly one Bond is stored at a time; a fresh pairing overwrites it.
_Avoid_: Pairing (see below), credential

**Pairing**:
The act of a phone and the device negotiating encryption for the first time. A completed pairing only produces a persisted Bond when it happens inside an armed Bondable Window; otherwise it's a transient encrypted link that leaves no Bond behind.
_Avoid_: Bonding (use Bond for the noun, Pairing for the verb/event)

**Bondable Window**:
The timed period (default 45s) after a GP22 button press during which a new Pairing is allowed to produce a persisted Bond. Outside the window, connections are still encryptable but not bondable.
_Avoid_: Pairing mode, admission window

**Wall Clock**:
The local time of day shown on the grid as 24-hour `HH:MM` (colon blinking once a second), derived from the last Sync's UTC plus the TZ Rule. What `CLOCK` displays.
_Avoid_: Elapsed time, uptime (the old boot-relative clock this replaced)

**Sync**:
One successful SNTP exchange that sets the device's UTC offset. Attempted once the Wi-Fi link has a DHCP lease, then every 6h; between Syncs the Wall Clock free-runs on the last one.
_Avoid_: NTP update, refresh

**Unsynced**:
The state before the first Sync since boot, when there's no UTC to show: the grid shows `--:--` with a solid colon, and `TIME` replies `ERR not synced`.

**TZ Rule**:
The persisted POSIX TZ string (e.g. `PST8PDT,M3.2.0,M11.1.0`) that converts UTC to local time, including DST transitions. Set by `TZ`; defaults to UTC when never set. See `docs/adr/0003-posix-tz-rule-on-device.md`.
_Avoid_: Timezone (ambiguous with IANA names like `America/Los_Angeles`, which the device never sees), offset
