import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter_blue_plus/flutter_blue_plus.dart';

import 'ble_central.dart';

/// The firmware's custom command service and its two characteristics
/// (`pico_w_display/src/bt.rs`'s `CommandService`).
final serviceUuid = Guid('e3fcb01d-9492-4fa7-97db-63f3491b3f58');
final commandUuid = Guid('e7880fa0-ab1f-4d65-bf1b-88ef1176393b');
final replyUuid = Guid('cce376e9-e3c2-41e1-8367-6c2bcf55784f');

/// The name the firmware advertises (`bt.rs`'s `DEVICE_NAME`), which is also
/// what the phone's Bluetooth settings list it as.
const deviceName = 'Plasma 2350 W';

/// How long a direct reconnect to the known device may take before it's
/// reported Unreachable — short, since at launch the user is waiting on it.
const _connectTimeout = Duration(seconds: 15);

/// How long to wait for the adapter to report a settled state. Generous,
/// because on a fresh install the first radio call raises the OS Bluetooth
/// permission prompt and the state stays `unknown` until it's answered.
const _adapterSettleTimeout = Duration(seconds: 60);

/// [BleCentral] over `flutter_blue_plus`. Deliberately thin: everything
/// worth testing lives in `DeviceConnection`.
class FlutterBlueCentral implements BleCentral {
  @override
  Future<String?> scanForDevice(Duration timeout) async {
    await _awaitAdapterOn();
    final found = Completer<String?>();
    final subscription = FlutterBluePlus.onScanResults.listen((results) {
      if (results.isNotEmpty && !found.isCompleted) {
        found.complete(results.last.device.remoteId.str);
      }
    });
    FlutterBluePlus.cancelWhenScanComplete(subscription);
    try {
      await FlutterBluePlus.startScan(
        withServices: [serviceUuid],
        timeout: timeout,
      );
      final scanEnded = FlutterBluePlus.isScanning
          .where((scanning) => !scanning)
          .first;
      return await Future.any([found.future, scanEnded.then((_) => null)]);
    } finally {
      await FlutterBluePlus.stopScan();
    }
  }

  @override
  Future<BleLink> connect(String remoteId) async {
    await _awaitAdapterOn();
    final device = BluetoothDevice.fromId(remoteId);
    // Personal/hobby use: see the FlutterBluePlus License.
    await device.connect(license: License.nonprofit, timeout: _connectTimeout);
    return FlutterBlueLink(device);
  }

  /// `adapterState` starts out `unknown` on iOS until CoreBluetooth reports
  /// in, so wait for a settled state rather than reading it immediately.
  Future<void> _awaitAdapterOn() async {
    final state = await FlutterBluePlus.adapterState
        .where(
          (s) =>
              s != BluetoothAdapterState.unknown &&
              s != BluetoothAdapterState.turningOn,
        )
        .first
        .timeout(
          _adapterSettleTimeout,
          onTimeout: () => BluetoothAdapterState.unknown,
        );
    if (state != BluetoothAdapterState.on) {
      throw BluetoothUnavailable(state);
    }
  }
}

/// The phone's Bluetooth adapter isn't usable (off, denied, or absent).
class BluetoothUnavailable implements Exception {
  const BluetoothUnavailable(this.state);

  final BluetoothAdapterState state;

  @override
  String toString() => switch (state) {
    BluetoothAdapterState.unauthorized => 'Bluetooth permission was denied.',
    BluetoothAdapterState.unavailable => 'This phone has no Bluetooth LE.',
    _ => 'Bluetooth is off.',
  };
}

/// [BleLink] over one `flutter_blue_plus` device.
class FlutterBlueLink implements BleLink {
  FlutterBlueLink(this._device);

  final BluetoothDevice _device;

  @override
  Future<void> secure({required bool pairing}) async {
    if (pairing && Platform.isAndroid) {
      // Android won't raise its Pairing dialog on its own for an
      // insufficient-encryption error; ask for it up front. A no-op if the
      // OS already holds a Bond for this device.
      await _device.createBond();
    }
    final services = await _device.discoverServices();
    final service = services.where((s) => s.uuid == serviceUuid).firstOrNull;
    if (service == null) {
      throw Exception('This is not a $deviceName display.');
    }
    final command = service.characteristics.firstWhere(
      (c) => c.uuid == commandUuid,
    );
    final reply = service.characteristics.firstWhere(
      (c) => c.uuid == replyUuid,
    );

    // `command` requires an encrypted link, so this write is what forces
    // encryption (and on iOS raises the Pairing dialog). Subscribing to
    // `reply` can't do that job: its CCCD is writable unencrypted in
    // `trouble-host`. A lone `\n` is an empty line, which the firmware's
    // `read_line` skips without dispatching a Command or sending a reply.
    await command.write([0x0a]);
    await reply.setNotifyValue(true);
  }

  @override
  Future<void> disconnect() => _device.disconnect();

  @override
  Future<void> get onDropped => _device.connectionState
      .where((s) => s == BluetoothConnectionState.disconnected)
      .first;
}
