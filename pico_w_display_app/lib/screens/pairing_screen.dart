import 'package:flutter/material.dart';

import '../ble/device_connection.dart';
import '../ble/flutter_blue_central.dart' show deviceName;

/// Walks the user through Pairing: press the device's button to open its
/// Bondable Window, then scan, connect and accept the OS Pairing dialog.
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
              '1. Press the button on the display.',
              style: theme.textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            Text(
              'This lets a new phone pair for the next ${bondableWindow.inSeconds} seconds. '
              'Pairing this phone replaces any phone paired before it.',
            ),
            const SizedBox(height: 24),
            Text(
              '2. Right away, tap Pair and accept the pairing request.',
              style: theme.textTheme.titleMedium,
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
        'If this display was paired before (or was UNPAIRed), first forget '
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
