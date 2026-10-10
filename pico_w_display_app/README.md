# pico_w_display_app

Flutter (iOS + Android) companion app for the `pico_w_display` firmware: pairs with the display over BLE, reconnects to it automatically on later launches, sends it Commands (`TEXT`, `CLOCK`, `COLOR`, `BRIGHTNESS`, `WIFI`, `RESET`), and keeps the display's TZ Rule on the phone's timezone (`TZ`, `TIME`). Domain terms (Pairing, Bond, Claimed, Factory Reset, …) follow `../pico_w_display/CONTEXT.md`.

## Layout

- `lib/ble/device_connection.dart` — the app's single device: launch-time choice between Pairing and a direct reconnect, and the Pairing flow itself. Unit-tested against a fake radio in `test/ble/`.
- `lib/ble/ble_central.dart` — the narrow BLE interface `DeviceConnection` drives; `flutter_blue_central.dart` implements it over `flutter_blue_plus`.
- `lib/ble/*known_device_store.dart` — persists the paired device's remote ID (`shared_preferences`).
- `lib/protocol.dart` — the Command grammar as pure encode/parse functions (plus the input validators the control screen uses), mirroring `pico_w_display/src/protocol.rs`. Unit-tested in `test/protocol_test.dart`.
- `lib/ble/command_client.dart` — sends Commands over a secured link and matches each `reply` notification to its Command. Unit-tested against a fake link.
- `lib/tz/` — the phone's timezone as a TZ Rule. `tz_rules.dart` looks the IANA zone up in the generated `posix_table.g.dart`, with a fixed-offset fallback for zones it lacks; `tz_rule_updater.dart` sends `TZ` then `TIME` on every connect and on app resume (`TZ` only when the rule changed), and reports the outcome for the control screen's status line. Unit-tested in `test/tz/`.
- `lib/screens/pairing_screen.dart` — the Pairing walkthrough.
- `lib/screens/control_screen.dart` — one control per Command, each showing the display's `OK`/`ERR` reply inline. `link_banner.dart` is the connection/bond-state strip over every screen.

## Develop

```sh
flutter test
flutter analyze
flutter run
```

### Regenerating the timezone table

`lib/tz/posix_table.g.dart` and the firmware's `../pico_w_display/testdata/tz_rules.txt` fixture are generated from a pinned IANA tzdata release (version in the file header). Re-run when a tzdata release changes rules that matter (see its NEWS), then run both test suites — the firmware's checks it accepts every rule:

```sh
dart run tool/gen_tz_table.dart 2026e   # needs curl, tar and zic
(cd ../pico_w_display && cargo test --lib --target aarch64-apple-darwin)
```

The tool fails if a rule is one the firmware rejects (over 48 bytes, or a `Jn`/`n` day form). A zone the phone reports that isn't in the table still gets a fixed-offset rule, so a stale table degrades rather than breaks.

`flutter_blue_plus` is used under its FlutterBluePlus License as personal use (`License.nonprofit` in `flutter_blue_central.dart`); it sends a small license ping at build time.

Never raise `flutter_blue_plus`'s log level to `LogLevel.verbose`: at that level it prints every written value, including `WIFI` passwords.

## Manual verification (Pairing)

1. Fresh install, freshly reset display (Unclaimed): tap Pair, no button press. Accept the OS pairing request. The app shows Connected, and the firmware logs `ble pairing complete` then `claimed`.
2. Kill and relaunch the app: it reconnects with no scan, no button press and no dialog.
3. Power-cycle the display, relaunch: same as 2.
4. Pair a second phone with the now-Claimed display: Pairing may complete on that phone, but the display persists no Bond, so step 3 then fails for it. (Until the Link Key slice, that phone's transient link can still run Commands.)
5. Repeat 1–3 on both a real iOS and a real Android device.

If Pairing fails after the display was Factory Reset or paired with another phone, forget "Plasma 2350 W" in the phone's Bluetooth settings first — the phone otherwise keeps offering the stale keys.

## Manual verification (control screen)

Against a real, bonded display:

1. Message: `12:34` → Show: the display shows 12:34 and the app shows `OK`. Resume clock: the clock returns, `OK`.
2. Color: pick a preset or adjust R/G/B → Set color: the glyphs change color, `OK`.
3. Brightness: drag and release the slider: the display dims/brightens, `OK`.
4. Wi-Fi: a correct SSID/password → `OK <address>`. Then a deliberately wrong password → `ERR wifi join failed` (or `ERR wifi join timed out`), and the password field is cleared. Check `flutter run`'s console and the platform logs (`adb logcat` / Xcode console) contain no trace of either password.
5. Power off the display while on the control screen: the banner and screen switch to "Paired · not connected"; Retry reconnects to the control screen.
6. Device → Factory reset → Cancel: nothing is sent. Factory reset → Reset: `OK`, the app returns to Pairing, and the display restarts showing `--:--` in the boot color. After it restarts it has no Bond (forget "Plasma 2350 W" in the phone's Bluetooth settings, then step 1 of Pairing works again), doesn't Rejoin Wi-Fi, and `TIME` reports `UTC0`.

## Manual verification (Button A)

1. Hold A: a bar fills across the middle of the grid over 5s, then the display restarts, with the same result as step 6 above.
2. Hold A for about 3s and let go: the bar disappears and the clock comes back; nothing is erased.
3. Tap A: nothing happens.
4. Press the BOOTSEL button (GP22) while the display is running: nothing happens.

## Manual verification (timezone)

Against a real, bonded display with Wi-Fi:

1. Connect: the status line reads `Synced · HH:MM <abbr>` matching the phone's clock, and the display shows the same local time.
2. Change the phone's timezone (Settings → turn off automatic time zone, pick another), return to the app: the display moves to that zone's time within a second, and the status line follows.
3. With the display off Wi-Fi (`FORGET`, then power-cycle): the status line reads `Waiting for network time`.
