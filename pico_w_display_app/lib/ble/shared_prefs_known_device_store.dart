import 'package:shared_preferences/shared_preferences.dart';

import 'known_device_store.dart';

const _key = 'knownDeviceRemoteId';

/// [KnownDeviceStore] over `shared_preferences`.
class SharedPrefsKnownDeviceStore implements KnownDeviceStore {
  @override
  Future<String?> load() => SharedPreferencesAsync().getString(_key);

  @override
  Future<void> save(String remoteId) =>
      SharedPreferencesAsync().setString(_key, remoteId);
}
