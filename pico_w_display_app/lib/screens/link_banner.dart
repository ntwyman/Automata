import 'package:flutter/material.dart';

import '../ble/device_connection.dart';

/// A strip across the top of every screen saying where the app stands with
/// its display, from the same [LinkState] that picks which screen shows.
class LinkBanner extends StatelessWidget {
  const LinkBanner({super.key, required this.state});

  final LinkState state;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final (icon, label, background, foreground) = switch (state) {
      Connected() => (
        Icons.bluetooth_connected,
        'Connected · paired',
        colors.primaryContainer,
        colors.onPrimaryContainer,
      ),
      // Not yet known whether a device was ever paired.
      Starting() => (
        Icons.bluetooth,
        'Starting…',
        colors.secondaryContainer,
        colors.onSecondaryContainer,
      ),
      Reconnecting() => (
        Icons.bluetooth_searching,
        'Reconnecting…',
        colors.secondaryContainer,
        colors.onSecondaryContainer,
      ),
      Unreachable() => (
        Icons.bluetooth_disabled,
        'Paired · not connected',
        colors.errorContainer,
        colors.onErrorContainer,
      ),
      Scanning() || Securing() || PairingFailed() => (
        Icons.bluetooth,
        'Pairing',
        colors.surfaceContainerHighest,
        colors.onSurfaceVariant,
      ),
      NeedsPairing() => (
        Icons.bluetooth,
        'Not paired',
        colors.surfaceContainerHighest,
        colors.onSurfaceVariant,
      ),
    };
    return Material(
      color: background,
      child: SafeArea(
        bottom: false,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
          child: Row(
            children: [
              Icon(icon, size: 16, color: foreground),
              const SizedBox(width: 8),
              Text(label, style: TextStyle(color: foreground)),
            ],
          ),
        ),
      ),
    );
  }
}
