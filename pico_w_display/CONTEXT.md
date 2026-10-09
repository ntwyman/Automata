# pico_w_display

Firmware for the Plasma 2350 W LED matrix: shows a Wall Clock in a choice of Faces, controlled from one Claimed phone, over Wi-Fi when it can and BLE for onboarding and recovery.

## Language

### Control

**Transport**:
A channel a client can open a Session over: the BLE GATT link, or a Wi-Fi TCP connection over the Secure Channel. `run_session` is written once against `embedded_io_async::Read`/`Write` and is identical regardless of which Transport carries it.
_Avoid_: Connection type, interface

**Session**:
One client's connected lifetime on a Transport: read a command line, dispatch it, wait for the display to ack, write back a reply. Ends when the Transport errors or closes; the caller then waits for a new one.
_Avoid_: Connection, request

**Command**:
A parsed instruction a Session dispatches to the display loop: `COLOR`, `BRIGHTNESS`, `FACE`, `NIGHT`, `WIFI`, `FORGET`, `TZ`, `TIME`, `RESET`, `KEY`. Transport-agnostic (the display loop only ever sees a `Command`, never which Transport it arrived over), with one exception: `KEY` is refused on every Transport but BLE, so the Link Key never leaves over Wi-Fi.

### Ownership

**Claimed**:
The state of a device that has a Bond, and so belongs to exactly one phone. An **Unclaimed** device (no Bond) accepts the first Pairing as its Bond; a Claimed one refuses every BLE link not encrypted against its Bond. Only a Factory Reset returns a device to Unclaimed. See `docs/adr/0005-first-come-claiming.md`.
_Avoid_: Owned, provisioned, locked

**Pairing**:
The act of a phone and the device negotiating encryption for the first time. Produces a persisted Bond only while the device is Unclaimed. That Pairing is what Claims it.
_Avoid_: Bonding (use Bond for the noun, Pairing for the verb/event)

**Bond**:
The persisted cryptographic pairing record (identity + LTK) that lets the Claimed phone re-establish an encrypted BLE link with no user action. Exactly one exists while Claimed, and none while Unclaimed.
_Avoid_: Pairing (see above), credential

**Link Key**:
The random 256-bit secret the device generates when it's Claimed, which the phone fetches over BLE with `KEY` and then uses to open the Secure Channel. One per Claim; replaced only by a Factory Reset. Stored in plaintext on the device, in the OS keystore on the phone. See `docs/adr/0007-plaintext-link-key.md`.
_Avoid_: Password, token, PSK (the role it plays, not its name)

**Secure Channel**:
The encrypted, mutually authenticated wrapper around a Wi-Fi Session: a Noise `NNpsk0` handshake keyed by the Link Key, then one encrypted frame per line. A client without the Link Key never gets a Session. See `docs/adr/0006-noise-secure-channel-not-tls.md`.
_Avoid_: TLS, encryption layer

**Factory Reset**:
Wiping every persisted item (Bond, Link Key, Saved Network, TZ Rule, display settings) and rebooting Unclaimed. Triggered by holding Button A for 5s or by `RESET`.
_Avoid_: Unpair, wipe, reset (unqualified)

### Network

**Saved Network**:
The persisted SSID + password from the last `WIFI` join that succeeded (`OK <addr>`). Exactly one is stored; a failed `WIFI` leaves it untouched, and `FORGET` (or a Factory Reset) clears it. Stored in plaintext. See `docs/adr/0004-plaintext-saved-network.md`.
_Avoid_: Credentials, profile

**Rejoin**:
The device's own join to its Saved Network, with no client involved: at boot, and whenever it's without a DHCP lease after that (the link dropped, or a join never got one), retrying with backoff (10s, 30s, 1m, then every 5m) for as long as a Saved Network exists. Distinct from a client's `WIFI` join.
_Avoid_: Reconnect, auto-join

### Time

**Wall Clock**:
The local time of day, derived from the last Sync's UTC plus the TZ Rule. It's what the grid always shows, drawn in the current Face.
_Avoid_: Elapsed time, uptime (the old boot-relative clock this replaced)

**Sync**:
One successful SNTP exchange that sets the device's UTC offset. Attempted once the Wi-Fi link has a DHCP lease, then every 6h; between Syncs the Wall Clock free-runs on the last one.
_Avoid_: NTP update, refresh

**Unsynced**:
The state before the first Sync since boot, when there's no UTC to show: each Face draws its placeholder (`--:--` with a solid colon, or rim markers with no hands), `TIME` replies `ERR not synced`, and the Night Window is ignored.

**TZ Rule**:
The persisted POSIX TZ string (e.g. `PST8PDT,M3.2.0,M11.1.0`) that converts UTC to local time, including DST transitions. Set by `TZ`; defaults to UTC when never set. See `docs/adr/0003-posix-tz-rule-on-device.md`.
_Avoid_: Timezone (ambiguous with IANA names like `America/Los_Angeles`, which the device never sees), offset

### Display

**Face**:
How the Wall Clock is drawn on the grid: **Digital** (24-hour `HH:MM`, colon blinking once a second) or **Analog** (hour and minute hands, rim markers, an orbiting seconds pixel). Persisted; chosen by `FACE` or a short press of Button A.
_Avoid_: Mode, screen, theme

**Night Window**:
The persisted daily span of local time (which may cross midnight) during which the grid drops to a set night brightness, 0 meaning off. Outside it the grid uses the **Day Brightness**, the last `BRIGHTNESS` sent outside the window; a `BRIGHTNESS` sent inside it lasts only until the window's next edge.
_Avoid_: Schedule, quiet hours, night mode
