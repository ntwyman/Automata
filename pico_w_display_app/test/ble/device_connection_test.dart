import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/ble/ble_central.dart';
import 'package:pico_w_display_app/ble/device_connection.dart';
import 'package:pico_w_display_app/ble/known_device_store.dart';
import 'package:pico_w_display_app/ble/link_key_store.dart';
import 'package:pico_w_display_app/protocol.dart';

const keyHex =
    '0109111921293139414951596169717981899199a1a9b1b9c1c9d1d9e1e9f1f9';
final key = parseLinkKey(keyHex);
final staleKey = parseLinkKey('f' * 64);

void main() {
  late FakeBleCentral central;
  late InMemoryKnownDeviceStore store;
  late InMemoryLinkKeyStore linkKeys;
  late DeviceConnection connection;

  setUp(() {
    central = FakeBleCentral();
    store = InMemoryKnownDeviceStore();
    linkKeys = InMemoryLinkKeyStore();
    connection = DeviceConnection(
      central: central,
      store: store,
      linkKeys: linkKeys,
    );
  });

  test('a fresh install needs Pairing and touches no radio', () async {
    await connection.start();

    expect(connection.state.value, isA<NeedsPairing>());
    expect(central.scans, isEmpty);
    expect(central.connects, isEmpty);
  });

  test(
    'a relaunch reconnects straight to the known device without scanning',
    () async {
      store.id = 'AA:BB';
      final link = central.deviceAt('AA:BB');

      await connection.start();

      expect(central.scans, isEmpty);
      expect(central.connects, ['AA:BB']);
      expect(link.encrypted, isTrue);
      expect(link.securedForPairing, isFalse);
      expect(connection.state.value, isA<Connected>());
    },
  );

  test('an unreachable known device is reported, not forgotten', () async {
    store.id = 'AA:BB';

    await connection.start();

    expect(connection.state.value, isA<Unreachable>());
    expect(central.scans, isEmpty);
    expect(store.id, 'AA:BB');
  });

  test(
    'retrying an Unreachable device reports Reconnecting, then Connected',
    () async {
      store.id = 'AA:BB';
      await connection.start();
      central.deviceAt('AA:BB');
      final seen = recordStates(connection);

      await connection.start();

      expect(seen, [Reconnecting, Connected]);
    },
  );

  test('choosing to pair afresh keeps the known device until a new Pairing succeeds', () async {
    store.id = 'AA:BB';
    await connection.start();

    connection.choosePairing();
    expect(connection.state.value, isA<NeedsPairing>());
    await connection.pair();

    expect(connection.state.value, isA<PairingFailed>());
    expect(store.id, 'AA:BB');
  });

  test('Pairing finds the device, secures it, and remembers it', () async {
    final link = central.deviceAt('CC:DD');
    central.advertising = 'CC:DD';

    await connection.pair();

    expect(central.scans, [const Duration(seconds: 45)]);
    expect(central.connects, ['CC:DD']);
    expect(link.encrypted, isTrue);
    expect(link.securedForPairing, isTrue);
    expect(store.id, 'CC:DD');
    expect(connection.state.value, isA<Connected>());
  });

  test(
    'Pairing reports its progress: scanning, then securing, then connected',
    () async {
      central.deviceAt('CC:DD');
      central.advertising = 'CC:DD';
      final seen = recordStates(connection);

      await connection.pair();

      expect(seen, [Scanning, Securing, Connected]);
    },
  );

  test(
    'Pairing with no device advertising fails and remembers nothing',
    () async {
      await connection.pair();

      expect(central.connects, isEmpty);
      expect(store.id, isNull);
      expect(connection.state.value, isA<PairingFailed>());
    },
  );

  test('Pairing when scanning itself fails (e.g. Bluetooth off) reports PairingFailed', () async {
    central.scanError = Exception('Bluetooth is off');

    await connection.pair();

    expect(connection.state.value, isA<PairingFailed>());
    expect(store.id, isNull);
  });

  test('Pairing that is refused during Pairing drops the link and remembers nothing', () async {
    final link = central.deviceAt('CC:DD')..refusesEncryption = true;
    central.advertising = 'CC:DD';

    await connection.pair();

    expect(link.disconnected, isTrue);
    expect(store.id, isNull);
    expect(connection.state.value, isA<PairingFailed>());
  });

  test(
    'forgetting the device after a Factory Reset returns to Pairing, for good',
    () async {
      store.id = 'AA:BB';
      final link = central.deviceAt('AA:BB');
      await connection.start();

      await connection.forgetDevice();
      // The device reboots, dropping the link: that's not Unreachable.
      link.drop();
      await pumpEventQueue();

      expect(connection.state.value, isA<NeedsPairing>());
      expect(link.disconnected, isTrue);
      expect(store.id, isNull);
      await connection.start();
      expect(connection.state.value, isA<NeedsPairing>());
    },
  );

  group('Link Key', () {
    test('Pairing fetches the Link Key with KEY and stores it', () async {
      final link = central.deviceAt('CC:DD');
      central.advertising = 'CC:DD';

      await connection.pair();

      expect(link.written, ['KEY']);
      expect(linkKeys.key, key);
      expect(connection.state.value, isA<Connected>());
    });

    test('Pairing replaces a stale Link Key with the new Claim\'s', () async {
      linkKeys.key = staleKey;
      central.deviceAt('CC:DD');
      central.advertising = 'CC:DD';

      await connection.pair();

      expect(linkKeys.key, key);
    });

    test('a refused Pairing keeps the stored Link Key', () async {
      linkKeys.key = staleKey;
      central.deviceAt('CC:DD').refusesEncryption = true;
      central.advertising = 'CC:DD';

      await connection.pair();

      expect(linkKeys.key, staleKey);
    });

    test('a reconnect with a stored Link Key sends no KEY', () async {
      store.id = 'AA:BB';
      linkKeys.key = key;
      final link = central.deviceAt('AA:BB');

      await connection.start();

      expect(link.written, isEmpty);
      expect(connection.state.value, isA<Connected>());
    });

    test('a reconnect with no stored Link Key asks for it again', () async {
      store.id = 'AA:BB';
      final link = central.deviceAt('AA:BB');

      await connection.start();

      expect(link.written, ['KEY']);
      expect(linkKeys.key, key);
    });

    test('a KEY refused by the device still connects, with no key', () async {
      store.id = 'AA:BB';
      central.deviceAt('AA:BB').keyReply = 'ERR not claimed';

      await connection.start();

      expect(linkKeys.key, isNull);
      expect(connection.state.value, isA<Connected>());
    });

    test('a garbled KEY reply still connects, with no key', () async {
      store.id = 'AA:BB';
      central.deviceAt('AA:BB').keyReply = 'OK 1234';

      await connection.start();

      expect(linkKeys.key, isNull);
      expect(connection.state.value, isA<Connected>());
    });

    test('forgetting the device forgets its Link Key', () async {
      store.id = 'AA:BB';
      central.deviceAt('AA:BB');
      await connection.start();

      await connection.forgetDevice();

      expect(linkKeys.key, isNull);
    });
  });

  test(
    'a link that drops after connecting leaves the known device Unreachable',
    () async {
      store.id = 'AA:BB';
      final link = central.deviceAt('AA:BB');
      await connection.start();

      link.drop();
      await pumpEventQueue();

      expect(connection.state.value, isA<Unreachable>());
    },
  );
}

/// Records the type of every [LinkState] [connection] moves through.
List<Type> recordStates(DeviceConnection connection) {
  final seen = <Type>[];
  connection.state.addListener(
    () => seen.add(connection.state.value.runtimeType),
  );
  return seen;
}

class InMemoryKnownDeviceStore implements KnownDeviceStore {
  String? id;

  @override
  Future<String?> load() async => id;

  @override
  Future<void> save(String remoteId) async => id = remoteId;

  @override
  Future<void> clear() async => id = null;
}

class InMemoryLinkKeyStore implements LinkKeyStore {
  LinkKey? key;

  @override
  Future<LinkKey?> load() async => key;

  @override
  Future<void> save(LinkKey key) async => this.key = key;

  @override
  Future<void> clear() async => key = null;
}

/// A device's link: answers `KEY` with [keyReply], and nothing else.
class FakeBleLink implements BleLink {
  bool refusesEncryption = false;
  bool encrypted = false;
  bool? securedForPairing;
  bool disconnected = false;
  String keyReply = 'OK $keyHex';
  final written = <String>[];
  final _replies = StreamController<String>.broadcast(sync: true);
  final _dropped = Completer<void>();

  void drop() => _dropped.complete();

  @override
  Future<void> get onDropped => _dropped.future;

  @override
  Future<void> secure({required bool pairing}) async {
    securedForPairing = pairing;
    if (refusesEncryption) throw Exception('insufficient authentication');
    encrypted = true;
  }

  @override
  Future<void> disconnect() async => disconnected = true;

  @override
  Future<void> writeLine(String line) async {
    written.add(line);
    if (line == 'KEY') scheduleMicrotask(() => _replies.add(keyReply));
  }

  @override
  Stream<String> get replyLines => _replies.stream;
}

class FakeBleCentral implements BleCentral {
  final scans = <Duration>[];
  final connects = <String>[];
  final _devices = <String, FakeBleLink>{};

  String? advertising;
  Exception? scanError;

  FakeBleLink deviceAt(String remoteId) => _devices[remoteId] = FakeBleLink();

  @override
  Future<String?> scanForDevice(Duration timeout) async {
    scans.add(timeout);
    if (scanError != null) throw scanError!;
    return advertising;
  }

  @override
  Future<BleLink> connect(String remoteId) async {
    connects.add(remoteId);
    final link = _devices[remoteId];
    if (link == null) throw Exception('$remoteId unreachable');
    return link;
  }
}
