import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/ble/ble_central.dart';
import 'package:pico_w_display_app/ble/device_connection.dart';
import 'package:pico_w_display_app/ble/known_device_store.dart';

void main() {
  late FakeBleCentral central;
  late InMemoryKnownDeviceStore store;
  late DeviceConnection connection;

  setUp(() {
    central = FakeBleCentral();
    store = InMemoryKnownDeviceStore();
    connection = DeviceConnection(central: central, store: store);
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

class FakeBleLink implements BleLink {
  bool refusesEncryption = false;
  bool encrypted = false;
  bool? securedForPairing;
  bool disconnected = false;
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
  Future<void> writeLine(String line) async {}

  @override
  Stream<String> get replyLines => const Stream.empty();
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
