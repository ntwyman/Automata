# Link Key stored in plaintext flash on the device, in the OS keystore on the phone

The device stores the Link Key in plaintext in the `SETTINGS_STORAGE` map, next to the Saved Network. ADR-0004's reasoning applies unchanged: someone with physical access can already read flash, or simply Factory Reset the device and claim it, so encrypting with a key that also lives in flash protects nothing. A real secret store (an OTP key plus secure boot) is a separate, much larger project.

The phone is different. Its storage is a realistic leak path (backups, other apps on a rooted device), so the app keeps the Link Key in the OS keystore via `flutter_secure_storage`, not in `shared_preferences`. Neither side ever logs the key, and the device never sends it over any Transport except BLE (`KEY`).
