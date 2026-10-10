import '../protocol.dart';

/// Persists the Claimed device's Link Key across launches, in the OS
/// keystore (`docs/adr/0007-plaintext-link-key.md`), never in plain prefs.
abstract interface class LinkKeyStore {
  Future<LinkKey?> load();
  Future<void> save(LinkKey key);

  /// Forgets the key, e.g. once the device has been Factory Reset.
  Future<void> clear();
}
