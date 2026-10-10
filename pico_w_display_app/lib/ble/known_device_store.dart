/// Persists the single known device's remote ID across launches, so a
/// relaunch reconnects directly instead of repeating Pairing.
abstract interface class KnownDeviceStore {
  Future<String?> load();
  Future<void> save(String remoteId);

  /// Forgets the device, e.g. once it has been Factory Reset.
  Future<void> clear();
}
