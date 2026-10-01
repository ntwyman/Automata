# pico_w_display_app

Flutter (iOS + Android) companion app for the `pico_w_display` firmware: pairs with the display over BLE and reconnects to it automatically on later launches. Domain terms (Pairing, Bond, Bondable Window, …) follow `../pico_w_display/CONTEXT.md`.

## Layout

- `lib/ble/device_connection.dart` — the app's single device: launch-time choice between Pairing and a direct reconnect, and the Pairing flow itself. Unit-tested against a fake radio in `test/ble/`.
- `lib/ble/ble_central.dart` — the narrow BLE interface `DeviceConnection` drives; `flutter_blue_central.dart` implements it over `flutter_blue_plus`.
- `lib/ble/*known_device_store.dart` — persists the paired device's remote ID (`shared_preferences`).
- `lib/screens/pairing_screen.dart` — the Pairing walkthrough.

## Develop

```sh
flutter test
flutter analyze
flutter run
```

`flutter_blue_plus` is used under its FlutterBluePlus License as personal use (`License.nonprofit` in `flutter_blue_central.dart`); it sends a small license ping at build time.

## Manual verification (Pairing)

1. Fresh install, display powered: tap Pair within 45s of pressing the display's button. Accept the OS pairing request. The app shows Connected, and the firmware logs `ble pairing complete` with a persisted Bond.
2. Kill and relaunch the app: it reconnects with no scan, no button press and no dialog.
3. Power-cycle the display, relaunch: same as 2.
4. Tap Pair *without* pressing the button first: Pairing may complete on the phone, but the display persists no Bond (ADR-0002's documented gap), so step 3 then fails.
5. Repeat 1–3 on both a real iOS and a real Android device.

If Pairing fails after the display was `UNPAIR`ed or paired with another phone, forget "Plasma 2350 W" in the phone's Bluetooth settings first — the phone otherwise keeps offering the stale keys.
