# BLE Support for `pico_w_display` + Flutter Companion App

## Context

Today the Plasma 2350 W display is controlled exclusively over a USB CDC-ACM serial link (`pico_w_display/src/usb.rs` + `protocol.rs`), so every routine change — set a message, pick a color, adjust brightness, join a new Wi-Fi network — requires physically tethering it to a computer. The board's CYW43439 combo chip already carries unused Bluetooth silicon. This plan adds a BLE GATT transport carrying the *exact same* line-based command protocol already defined transport-agnostically in `protocol.rs`, plus a new Flutter app (iOS + Android) as the day-to-day control surface. USB becomes a fallback/dev path rather than the only path. Pairing is bonded/encrypted, gated behind a physical button press so a stranger can't silently bond, but an already-paired phone reconnects automatically with no button press — since requiring a cable or a button for routine use would undercut the entire point of the feature.

All facts below (dependency versions, exact API shapes, pin mappings) were verified live against the real crates/repos/hardware docs during a research + grilling session with the user, not recalled from training data — cited inline so they're easy to re-check.

## Decisions locked in (from the grilling session)

1. USB stays as a fallback/dev path; BLE is the primary path for the app. Additive, not a rewrite.
2. BLE exposes the *exact* same command set as USB today: `TEXT`, `CLOCK`, `COLOR`, `BRIGHTNESS`, `WIFI`. No new commands in v1.
3. App manages a single device.
4. Flutter targets both iOS and Android, org id `com.conky.automata`.
5. New app lives at `pico_w_display_app/` in this repo, alongside `pico_w_display/` and `ws_2812_level_shift/`.
6. BLE pairing is bonded/encrypted (not open).
7. Device always advertises/is connectable (no button needed to *reconnect*). A button press (GP22) is only required to admit a **new** bond; outside that window, an unknown peer cannot obtain a persisted bond.
8. Single bond slot: a fresh pairing replaces the old bond.
9. `UNPAIR` is a new USB serial command that clears the stored bond. It's fine that it's technically also reachable over BLE (shared `run_session` code path) — an attacker would already need to be bonded/encrypted to reach it, at which point they already have full control.
10. Proceed now with git-pinned/patched `cyw43` + `trouble-host` dependencies (matching the official embassy example) rather than waiting for a clean crates.io resolution. The known open upstream WiFi+BLE concurrency bug (embassy-rs/embassy#7081) is an accepted, monitored risk, not a reason to avoid running both radios together.
11. Delete the stale `feature/bluetooth` branch (confirmed zero unique commits vs. `main`, no remote counterpart) as part of this work.
12. Accept the "transient non-bonded pairing" gap for v1 (see §3): an unarmed connection's SMP pairing can still run to completion as a transient, non-persisted encrypted link, since `trouble-host` has no pre-negotiation reject hook. This matches decision #6's actual intent — block *persistent* stranger access, not resist an adversary already targeting the device over the air — so no `disconnect()`-on-`PairingComplete{bond:None}` guard is added.

## 1. Firmware: `pico_w_display/Cargo.toml`

Add the patch block used by the official `embassy-rs/trouble` `rp-pico-2-w` example (RP2350 — the correct sibling for this board, not the RP2040 `rp-pico-w` example):

```toml
[patch.crates-io]
cyw43 = { git = "https://github.com/embassy-rs/embassy.git", rev = "3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a" }
cyw43-pio = { git = "https://github.com/embassy-rs/embassy.git", rev = "3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a" }
embassy-rp = { git = "https://github.com/embassy-rs/embassy.git", rev = "3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a" }
embassy-sync = { git = "https://github.com/embassy-rs/embassy.git", rev = "3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a" }
embassy-executor = { git = "https://github.com/embassy-rs/embassy.git", rev = "3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a" }
embassy-time = { git = "https://github.com/embassy-rs/embassy.git", rev = "3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a" }
```

`embassy-futures` and `embassy-net` stay on crates.io as today (not in the patch list).

Dependency edits:
- `cyw43 = { version = "0.7.0", features = ["defmt", "bluetooth"] }` — add the `bluetooth` feature.
- Add `trouble-host = { version = "0.8", default-features = false, features = ["derive", "gatt", "peripheral", "security"] }`. Try the plain crates.io version first — the patched `cyw43`/`cyw43-pio` move to `bt-hci ^0.10`, matching published `trouble-host 0.8.0`'s requirement, so this should resolve without needing to also git-pin `trouble-host`. Fall back to a git pin only if it doesn't resolve in practice.
- Bond persistence: `sequential-storage = { version = "7.1", features = ["postcard"] }`, `embedded-storage-async = "0.4.1"`, `serde = { version = "1", default-features = false, features = ["derive"] }` — matches what `trouble-host`'s own bonding example uses.
- Not actionable, just so it isn't a surprise mid-build: `trouble-host`'s `security` feature pulls its own git dependency on `embassy-crypto` pinned to a *different* commit than the `[patch.crates-io]` block above — expected, it's a distinct crate name so Cargo resolves it separately.

## 2. Firmware: Bluetooth bring-up (`wifi.rs` extension)

`wifi.rs`'s current `init()` (`pico_w_display/src/wifi.rs:80-126`) calls `cyw43::new(state, pwr, spi, fw, nvram)` on PIO1/DMA_CH1 and spawns one `cyw43_task`. Swap this for `cyw43::new_with_bluetooth(state, pwr, spi, fw, btfw, nvram)`, which returns a network device, a `bt_device` handle, `Control`, and a single `Runner` that drives **both** radios — no second task needed, `cyw43_task`'s signature just gains a third generic parameter.

- **New firmware blob required**: `43439A0_btfw.bin`, not currently vendored in `pico_w_display/cyw43-firmware/` (today only `43439A0.bin`, `43439A0_clm.bin`, `nvram_rp2040.bin` exist). Pull it from `embassy-rs/embassy`'s `cyw43-firmware/` directory and load it via `aligned_bytes!` exactly like the other three (`wifi.rs:89-91`).
- Keep `wifi.rs::init()` as the single owner of chip/PIO/SPI bring-up (untouched pins PIN_23/24/25/29, untouched PIO1/DMA_CH1) — it now additionally returns the raw `bt_device` handle for a new `bt.rs` module to build the GATT peripheral on top of. `bt.rs` never touches PIO/SPI directly.
- Add a code comment at the `new_with_bluetooth(...)` call site linking `embassy-rs/embassy#7081` (panics/deadlocks running BLE + WiFi concurrently on Pico 2 W; a local fix exists upstream but isn't merged) — for future debugging context, not a design constraint per decision #10.

## 3. Firmware: GATT server (new `pico_w_display/src/bt.rs`)

Reference: `embassy-rs/trouble`'s `examples/apps/src/ble_bas_peripheral_bonding.rs`, verified live against the current release (`trouble-host` 0.8.0, 2026-08-25).

**Service/characteristic layout** — one custom GATT service (a fresh random 128-bit UUID, defined as a constant), two characteristics:
- `command` — `write`/`write_without_response`, `permissions(encrypted)`. One GATT write = one full command line's ASCII bytes (comfortably under the negotiated MTU; `protocol.rs::MAX_LINE_LEN` is 104 bytes).
- `reply` — `notify`, `permissions(encrypted)`. One notification = one full reply line (`protocol.rs`'s reply buffer is `String<40>`, `protocol.rs:225`, so it always fits one packet).

**Bridging to `protocol.rs::run_session` unmodified** — `run_session<R: Read, W: Write>` (`protocol.rs:207`) is deliberately transport-agnostic (see its own doc comment at `protocol.rs:1-5`). Don't touch it; add two small adapters in `bt.rs` instead:
- `BleReader: embedded_io_async::Read` — a per-connection task loops on `conn.next().await`; on a `GattEvent::Write` to `command`, it appends a synthetic trailing `\n` and pushes the bytes into a small `embassy_sync::channel::Channel<CriticalSectionRawMutex, heapless::Vec<u8, 104>, 2>` (mirrors the existing single-slot-mailbox style of `CommandChannel`/`AckChannel` at `protocol.rs:33-36`). `BleReader::read()` drains that buffer one byte at a time, refilling via `channel.receive().await` when exhausted — `read_line()`'s per-byte reads need nothing else.
- `BleWriter: embedded_io_async::Write` — holds the `GattConnection` and the `reply` characteristic; `write_all()` is a direct passthrough to `reply_characteristic.notify(conn, bytes, true).await`.
- Both are constructed fresh per connection and driven by a session future structurally parallel to `main.rs`'s existing USB loop (`main.rs:128-135`).

**Advertising & pairing model (this is the part that needed correcting mid-design — verified against `trouble-host` 0.8.0 source directly):**
- The device advertises continuously (`Advertisement::ConnectableScannableUndirected`, in an infinite loop, exactly like the reference example) — this alone gives "always connectable," satisfying reconnect-without-a-button.
- **Reconnecting an already-bonded phone needs zero extra logic.** Encryption resumption for a previously-bonded peer happens at the link layer via its stored LTK, matched against bonds registered with `stack.add_bond_information()` at boot — this path is entirely independent of the per-connection `bondable` flag (verified in `trouble-host`'s `security_manager/mod.rs`: `find_bond()`/`try_enable_bonded_encryption()` never consult it).
- **Admitting a *new* bond is gated by `conn.raw().set_bondable(is_window_armed())`**, called immediately after each `accept()`, before any pairing traffic arrives. `is_window_armed()` reads the flag armed by a GP22 press (see §5) with a timeout (default **45 seconds** — a tunable constant, not a hard architectural choice; bump it later if verification step 3 shows the app's pairing flow needs longer). Outside the window, connections default to `bondable(false)`.
- **Known limitation, worth documenting rather than hiding** (decision #12): `trouble-host` has no pre-negotiation "reject this pairing attempt" hook — only a post-hoc report of the outcome. With `bondable(false)`, an unarmed connection's SMP pairing can still *run to completion* as a transient (non-bondable, non-persisted) encrypted link — `PairingComplete { bond: None, .. }` — it just never gets a `BondInformation` to persist. Accepted as-is for v1; call it out in code as a documented, deliberate simplification. If tighter enforcement is ever wanted, add a guard in the connection-event loop: on `PairingComplete { bond: None, .. }` while unarmed, call `conn.raw().disconnect()` immediately.
- `CONNECTIONS_MAX = 1` in `HostResources` (matches decision #3/#8: single device, single bond slot).

**Bond persistence** (flash, following the reference bonding example's pattern):
- `pico_w_display/memory.x` currently defines a single `FLASH : ORIGIN = 0x10000000, LENGTH = 2048K` region with no carve-out. Add a reserved region at the end of flash — the last **16 KiB / 4 sectors** (RP2350's erase sector size is 4096 bytes), final not illustrative — exposing `__storage_start`/`__storage_end` symbols, mirroring `embassy-rs/trouble`'s `examples/nrf52/src/bin/ble_bas_peripheral_bonding.rs`.
- Drive it with `embassy_rp::flash::Flash::<Async, FLASH_SIZE>::new(p.FLASH, dma_ch)`, which implements `embedded_storage_async::nor_flash::NorFlash` (`ERASE_SIZE = 4096`, `WRITE_SIZE = 1`) — exactly what `sequential_storage::map` needs. Use a free DMA channel (e.g. `DMA_CH2`; `DMA_CH0` is WS2812, `DMA_CH1` is the CYW43439 SPI).
- Store the single bond record (`#[derive(Serialize, Deserialize)] struct StoredBondInformation(BondInformation)`) under a fixed constant key — storing under one key is itself what enforces the single-bond-slot policy on the storage side (a new pairing's `store_item` just overwrites the old entry).
- On `PairingComplete { bond: Some(bond), .. }`: persist the new bond to flash, and also call `Stack::remove_bond_information(old_identity)` for whatever was loaded at boot, so the live security manager doesn't end up holding two bonds alongside `CONNECTIONS_MAX = 1`.

## 4. Firmware: `UNPAIR` command (`protocol.rs` changes)

Follows the shape of the existing `CLOCK`/`WIFI` commands:
- Add `UNPAIR` to `parse_line`'s dispatch chain (`protocol.rs:86-98`), no arguments, like `CLOCK`.
- Since it touches flash storage owned by `bt.rs` (kept out of `protocol.rs` to preserve its deliberate transport-agnosticism, `protocol.rs:1-5`), thread a new handle into `run_session` alongside the existing `wifi: &mut Wifi` parameter (`protocol.rs:212`) — e.g. `bond_store: &mut BondStore` exposing `clear()`.
- New `ParsedLine`-adjacent variant handled in `run_session`'s match (`protocol.rs:238-257`): call `bond_store.clear().await` (erase the flash region) and evict the in-memory bond via `stack.remove_bond_information(identity)` if tracked, replying `OK` or `ERR <reason>`.
- Per decision #9, leave this reachable over BLE too rather than adding a transport-kind gate — simplest, and low-risk since reaching it over BLE already requires being bonded/encrypted.

## 5. Firmware: button handling (new `pico_w_display/src/button.rs`)

GP22 is the Plasma 2350 W's BOOT/user button, wired to a normal GPIO in addition to its bootloader function, hardware pulled high (Pimoroni product page + forum — moderate confidence, not schematic-sourced, so validate before relying on it — see verification step below).

- `Input::new(p.PIN_22, Pull::None)`, standard debounce idiom: `wait_for_falling_edge().await`, then a ~30ms settle check the level is still low, then a ~500ms cooldown before re-arming to reject bounce.
- On a debounced press, signal an `embassy_sync::signal::Signal<CriticalSectionRawMutex, ()>` that the BLE task reads to arm the 45s bondable window (`embassy_time::with_timeout`, mirroring `wifi.rs`'s existing `JOIN_TIMEOUT` pattern at `wifi.rs:31,54`).
- **Hardware sanity check before this is load-bearing**: flash a trivial standalone `pico_w_display/src/bin/button_check.rs` (auto-discovered by Cargo's `src/bin/*.rs` convention) that just inits `PIN_22` as `Input` and `defmt::info!`s on every press. Confirm over RTT (`probe-rs attach --chip RP235x`) before wiring GP22 into the real pairing logic.

## 6. Firmware: `main.rs` orchestration

Today: `join3(usb_fut, protocol_fut, display_fut)` (`main.rs:190`), with `wifi::init()` internally spawning tasks via `spawner.spawn()`. New shape:
- `wifi::init()` (per §2) also returns the `bt_device` handle, passed to a new `bt::init()` that builds the `trouble_host::Stack`, spawns the BLE host `Runner` task, and returns a session future analogous to the USB one.
- `button::watch(p.PIN_22)` becomes a new future.
- `commands`/`acks` channels (`protocol::CommandChannel`/`AckChannel`, `main.rs:123-124`) stay shared between the USB and BLE session futures — `display_fut` (`main.rs:137-188`) needs zero changes since it only ever sees `protocol::Command` values regardless of transport.
- Final join: `embassy_futures::join::join5(usb_fut, usb_protocol_fut, ble_session_fut, button_fut, display_fut)` — `join5` is already available via the existing unpatched `embassy-futures = "0.1.2"` dependency.

## 7. Flutter app: `pico_w_display_app/`

New top-level directory alongside `pico_w_display/` and `ws_2812_level_shift/`: `flutter create --org com.conky.automata --platforms=android,ios pico_w_display_app`.

- **Dependency**: `flutter_blue_plus` (standard BLE-central package, Android/iOS/macOS, stream-based API).
- **Structure**: `lib/ble/device_connection.dart` (scan/connect/subscribe for the single known device; store its remote ID via `shared_preferences` after first pairing so later launches reconnect directly), `lib/ble/protocol.dart` (encodes/decodes the same ASCII grammar as `protocol.rs`), `lib/screens/pairing_screen.dart`, `lib/screens/control_screen.dart`, `lib/main.dart`.
- **Pairing flow**: instructs the user to press the device's button, then scans (filtered by the custom service UUID from §3) during the window, connects, and triggers the OS-level bonding dialog by touching the `permissions(encrypted)` characteristics (`BluetoothDevice.createBond()` on Android; iOS bonds implicitly on first encrypted access). Stores the device ID on success.
- **Control screen**: five controls mapping 1:1 to the command set — `TEXT` (text field constrained to `DD:DD`) plus a "resume clock" button for bare `CLOCK`, a color picker → `RRGGBB` for `COLOR`, a 0–255 slider for `BRIGHTNESS`, and an SSID/password form for `WIFI` (password masked, never logged client-side, mirroring `protocol.rs`'s own redaction at `protocol.rs:233-234`). Each write awaits the next `reply` notification and surfaces `OK`/`ERR <reason>` inline. A connection/bond-state banner drives whether the pairing or control screen shows.
- **Protocol encoding** must exactly mirror `protocol.rs`'s grammar (`parse_text`/`parse_color`/`parse_brightness`/`parse_wifi`, `protocol.rs:103-161`): `WIFI` splits on the *first* space only (SSID can't contain spaces, password may), hex digits for `COLOR` may be either case.

## 8. Branch cleanup

`git branch -a` shows no remote counterpart for `feature/bluetooth` — a plain `git branch -d feature/bluetooth` is sufficient, nothing to delete remotely.

## 9. Verification plan

1. **GP22 sanity check first** (before it's load-bearing): flash `src/bin/button_check.rs`, press the button, confirm an RTT log line via `probe-rs attach --chip RP235x`.
2. **Build/flash**: `cargo build`; `.cargo/config.toml`'s active runner is `probe-rs run --chip RP235x` (the `picotool` line is present but commented out — note `pico_w_display/CLAUDE.md` is stale on this point, unrelated to this work).
3. **Pairing**: press GP22, confirm (via RTT) the bondable window opens; from the Flutter pairing screen, scan/connect/bond within the window; confirm bonding completes on both the phone (OS dialog) and device (`PairingComplete` log). Power-cycle the device and confirm the phone reconnects automatically with no button press — this is the key behavior this plan was revised around.
4. **Each of the 5 commands**: send `TEXT`, `CLOCK`, `COLOR`, `BRIGHTNESS`, `WIFI` from the app's control screen; confirm the LED matrix responds and the app surfaces `OK`/`OK <ip>`/`ERR <reason>` correctly (include a deliberately-wrong Wi-Fi password to confirm `ERR` surfaces and is never logged).
5. **`UNPAIR`**: send it over the USB serial fallback (e.g. `picocom`); confirm `OK`; confirm the previously-bonded phone can no longer reconnect without a fresh GP22-triggered pairing.
6. **WiFi + BLE concurrency smoke test**: with a bond established, join Wi-Fi via `WIFI` while a BLE connection is live, and separately trigger a BLE reconnect while Wi-Fi/DHCP is active. Watch RTT logs for the known embassy-rs/embassy#7081 signature — this only proves this specific simple usage pattern doesn't trip it, not that the upstream issue is generally absent, per the accepted-risk framing in decision #10.

## Critical files

- `pico_w_display/Cargo.toml`
- `pico_w_display/src/wifi.rs`
- `pico_w_display/src/protocol.rs`
- `pico_w_display/src/main.rs`
- `pico_w_display/memory.x`
- `pico_w_display/src/bt.rs` (new)
- `pico_w_display/src/button.rs` (new)
- `pico_w_display/src/bin/button_check.rs` (new, throwaway sanity-check binary)
- `pico_w_display_app/lib/ble/protocol.dart` (new)
- `pico_w_display_app/lib/ble/device_connection.dart` (new)
