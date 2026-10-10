import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import '../protocol.dart';
import 'link_key_store.dart';

const _key = 'linkKey';

/// [LinkKeyStore] over `flutter_secure_storage`: the iOS Keychain, or the
/// Android Keystore.
class SecureLinkKeyStore implements LinkKeyStore {
  static const _storage = FlutterSecureStorage();

  /// A stored value that isn't a Link Key reads as none, so the app asks
  /// the device again.
  @override
  Future<LinkKey?> load() async {
    final hex = await _storage.read(key: _key);
    if (hex == null) return null;
    try {
      return parseLinkKey(hex);
    } on FormatException {
      return null;
    }
  }

  @override
  Future<void> save(LinkKey key) => _storage.write(key: _key, value: key.hex);

  @override
  Future<void> clear() => _storage.delete(key: _key);
}
