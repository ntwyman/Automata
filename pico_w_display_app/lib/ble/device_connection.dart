import 'package:flutter/foundation.dart';

import 'ble_central.dart';
import 'known_device_store.dart';

/// Where the app stands with its single device; drives which screen shows.
sealed class LinkState {
  const LinkState();
}

/// No device is known yet: the app must walk the user through Pairing.
class NeedsPairing extends LinkState {
  const NeedsPairing();
}

/// An encrypted link to the known device is up.
class Connected extends LinkState {
  const Connected(this.link);

  final BleLink link;
}

/// A device is known but the link to it couldn't be opened or secured —
/// out of range, powered off, or its Bond was cleared. The known device is
/// kept; the user can retry or deliberately pair afresh.
class Unreachable extends LinkState {
  const Unreachable(this.reason);

  final String reason;
}

/// Pairing: scanning for the device's advertisement during the Bondable
/// Window.
class Scanning extends LinkState {
  const Scanning();
}

/// Pairing: found the device, connecting and waiting on the OS-level
/// Pairing dialog.
class Securing extends LinkState {
  const Securing();
}

/// The last Pairing attempt didn't complete; nothing was remembered.
class PairingFailed extends LinkState {
  const PairingFailed(this.reason);

  final String reason;
}

/// Before [DeviceConnection.start] has checked for a known device.
class Starting extends LinkState {
  const Starting();
}

/// Opening an encrypted link straight to the known device.
class Reconnecting extends LinkState {
  const Reconnecting();
}

/// Mirrors the firmware's Bondable Window (`pairing_window.rs`, default 45s):
/// a Pairing only yields a persisted Bond on the device if it completes
/// within this long of the GP22 button press, so there's no point scanning
/// for longer.
const bondableWindow = Duration(seconds: 45);

/// Owns the app's single device: decides at launch between Pairing and a
/// direct reconnect, and exposes where that stands as [state].
class DeviceConnection {
  DeviceConnection({required this._central, required this._store});

  final BleCentral _central;
  final KnownDeviceStore _store;

  final ValueNotifier<LinkState> state = ValueNotifier(const Starting());

  Future<void> start() async {
    final remoteId = await _store.load();
    if (remoteId == null) {
      state.value = const NeedsPairing();
      return;
    }
    state.value = const Reconnecting();
    try {
      _adopt(await _openSecureLink(remoteId, pairing: false));
    } catch (e) {
      state.value = Unreachable(_describe(e));
    }
  }

  /// Switches to the Pairing flow, e.g. to replace an Unreachable device.
  /// The known device is only replaced once a new Pairing succeeds.
  void choosePairing() => state.value = const NeedsPairing();

  /// Runs Pairing: call once the user has pressed the device's button.
  /// The device is only remembered once the link is encrypted, so an
  /// abandoned or refused Pairing leaves any previously known device as is.
  Future<void> pair() async {
    state.value = const Scanning();
    try {
      final remoteId = await _central.scanForDevice(bondableWindow);
      if (remoteId == null) {
        state.value = const PairingFailed(
          'No device found. Press its button and try again.',
        );
        return;
      }
      state.value = const Securing();
      final link = await _openSecureLink(remoteId, pairing: true);
      await _store.save(remoteId);
      _adopt(link);
    } catch (e) {
      state.value = PairingFailed(_describe(e));
    }
  }

  /// Makes [link] the current one, falling back to [Unreachable] if it drops.
  void _adopt(BleLink link) {
    final connected = Connected(link);
    state.value = connected;
    link.onDropped.then((_) {
      if (identical(state.value, connected)) {
        state.value = const Unreachable('Disconnected');
      }
    });
  }

  /// Connects to [remoteId] and brings the link up to encrypted, dropping it
  /// again if encryption fails so no half-open link is left behind.
  Future<BleLink> _openSecureLink(
    String remoteId, {
    required bool pairing,
  }) async {
    final link = await _central.connect(remoteId);
    try {
      await link.secure(pairing: pairing);
    } catch (_) {
      await link.disconnect();
      rethrow;
    }
    return link;
  }

  /// Error text fit for the UI: drops Dart's `Exception: ` prefix.
  static String _describe(Object e) =>
      '$e'.replaceFirst(RegExp(r'^Exception: '), '');
}
