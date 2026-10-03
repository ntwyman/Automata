# pico_w_display_app

Flutter (iOS + Android) companion app for the `pico_w_display` firmware: pairs with the display over BLE, reconnects to it automatically on later launches, and sends it Commands (`TEXT`, `CLOCK`, `COLOR`, `BRIGHTNESS`, `WIFI`). Domain terms (Pairing, Bond, Bondable Window, …) follow `../pico_w_display/CONTEXT.md`.

## Layout

- `lib/ble/device_connection.dart` — the app's single device: launch-time choice between Pairing and a direct reconnect, and the Pairing flow itself. Unit-tested against a fake radio in `test/ble/`.
- `lib/ble/ble_central.dart` — the narrow BLE interface `DeviceConnection` drives; `flutter_blue_central.dart` implements it over `flutter_blue_plus`.
- `lib/ble/*known_device_store.dart` — persists the paired device's remote ID (`shared_preferences`).
- `lib/protocol.dart` — the Command grammar as pure encode/parse functions (plus the input validators the control screen uses), mirroring `pico_w_display/src/protocol.rs`. Unit-tested in `test/protocol_test.dart`.
- `lib/ble/command_client.dart` — sends Commands over a secured link and matches each `reply` notification to its Command. Unit-tested against a fake link.
- `lib/screens/pairing_screen.dart` — the Pairing walkthrough.
- `lib/screens/control_screen.dart` — one control per Command, each showing the display's `OK`/`ERR` reply inline. `link_banner.dart` is the connection/bond-state strip over every screen.

## Develop

```sh
flutter test
flutter analyze
flutter run
```

`flutter_blue_plus` is used under its FlutterBluePlus License as personal use (`License.nonprofit` in `flutter_blue_central.dart`); it sends a small license ping at build time.

Never raise `flutter_blue_plus`'s log level to `LogLevel.verbose`: at that level it prints every written value, including `WIFI` passwords.

## Manual verification (Pairing)

1. Fresh install, display powered: tap Pair within 45s of pressing the display's button. Accept the OS pairing request. The app shows Connected, and the firmware logs `ble pairing complete` with a persisted Bond.
2. Kill and relaunch the app: it reconnects with no scan, no button press and no dialog.
3. Power-cycle the display, relaunch: same as 2.
4. Tap Pair *without* pressing the button first: Pairing may complete on the phone, but the display persists no Bond (ADR-0002's documented gap), so step 3 then fails.
5. Repeat 1–3 on both a real iOS and a real Android device.

If Pairing fails after the display was `UNPAIR`ed or paired with another phone, forget "Plasma 2350 W" in the phone's Bluetooth settings first — the phone otherwise keeps offering the stale keys.

## Manual verification (control screen)

Against a real, bonded display:

1. Message: `12:34` → Show: the display shows 12:34 and the app shows `OK`. Resume clock: the clock returns, `OK`.
2. Color: pick a preset or adjust R/G/B → Set color: the glyphs change color, `OK`.
3. Brightness: drag and release the slider: the display dims/brightens, `OK`.
4. Wi-Fi: a correct SSID/password → `OK <address>`. Then a deliberately wrong password → `ERR wifi join failed` (or `ERR wifi join timed out`), and the password field is cleared. Check `flutter run`'s console and the platform logs (`adb logcat` / Xcode console) contain no trace of either password.
5. Power off the display while on the control screen: the banner and screen switch to "Paired · not connected"; Retry reconnects to the control screen.
