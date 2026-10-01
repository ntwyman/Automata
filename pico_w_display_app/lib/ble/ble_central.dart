/// The BLE operations [DeviceConnection] needs from the phone's radio,
/// kept narrow so the Pairing/reconnect logic can be driven by a fake in
/// tests. The real adapter over `flutter_blue_plus` lives in
/// `flutter_blue_central.dart`.
abstract interface class BleCentral {
  /// Scans for the device's custom GATT service UUID for up to [timeout] and
  /// returns the first match's remote ID, or `null` if none showed up.
  Future<String?> scanForDevice(Duration timeout);

  /// Opens a link to [remoteId] directly, without scanning first.
  Future<BleLink> connect(String remoteId);
}

/// One open link to the device.
abstract interface class BleLink {
  /// Touches the encrypted `command` characteristic, forcing the link up to
  /// an encrypted one, then subscribes to `reply`. Against a known Bond this
  /// resumes silently; otherwise it triggers the OS-level Pairing dialog.
  /// [pairing] is true only inside the Pairing flow, where the adapter may
  /// prompt for a new Bond up front; a reconnect must never do that. Throws
  /// if encryption fails or is refused.
  Future<void> secure({required bool pairing});

  /// Deliberately closes the link.
  Future<void> disconnect();

  /// Completes once the link goes down, for whatever reason.
  Future<void> get onDropped;
}
