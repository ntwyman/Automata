import 'package:flutter/material.dart';

import '../ble/device_connection.dart';
import '../ble/flutter_blue_central.dart' show deviceName;

/// Walks the user through Pairing: scan, connect and accept the OS Pairing
/// dialog. The first phone to pair with an Unclaimed display Claims it.
class PairingScreen extends StatelessWidget {
  const PairingScreen({
    super.key,
    required this.connection,
    required this.state,
  });

  final DeviceConnection connection;
  final LinkState state;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('Pair your display')),
      body: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Tap Pair and accept the pairing request.',
              style: theme.textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            const Text(
              'A new or reset display belongs to the first phone that pairs '
              'with it. To pair a display that already belongs to a phone, '
              'Factory Reset it first: hold its A button for 5 seconds.',
            ),
            const SizedBox(height: 24),
            ..._progress(theme),
          ],
        ),
      ),
    );
  }

  List<Widget> _progress(ThemeData theme) => switch (state) {
    Scanning() => const [_Busy('Looking for the display…')],
    Securing() => const [
      _Busy('Connecting — accept the pairing request if your phone asks.'),
    ],
    PairingFailed(:final reason) => [
      Text(reason, style: TextStyle(color: theme.colorScheme.error)),
      const SizedBox(height: 8),
      const Text(
        'If this display was paired before (or was Factory Reset), first forget '
        '"$deviceName" in your phone\'s Bluetooth settings.',
      ),
      const SizedBox(height: 16),
      FilledButton(onPressed: connection.pair, child: const Text('Try again')),
    ],
    _ => [FilledButton(onPressed: connection.pair, child: const Text('Pair'))],
  };
}

class _Busy extends StatelessWidget {
  const _Busy(this.label);

  final String label;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      const SizedBox.square(
        dimension: 20,
        child: CircularProgressIndicator(strokeWidth: 2),
      ),
      const SizedBox(width: 16),
      Expanded(child: Text(label)),
    ],
  );
}
