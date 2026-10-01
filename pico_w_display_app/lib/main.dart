import 'package:flutter/material.dart';

import 'ble/device_connection.dart';
import 'ble/flutter_blue_central.dart';
import 'ble/shared_prefs_known_device_store.dart';
import 'screens/pairing_screen.dart';

void main() {
  final connection = DeviceConnection(
    central: FlutterBlueCentral(),
    store: SharedPrefsKnownDeviceStore(),
  );
  runApp(PicoWDisplayApp(connection: connection));
  connection.start();
}

class PicoWDisplayApp extends StatelessWidget {
  const PicoWDisplayApp({super.key, required this.connection});

  final DeviceConnection connection;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Plasma Display',
      theme: ThemeData(colorSchemeSeed: Colors.deepPurple),
      darkTheme: ThemeData(
        colorSchemeSeed: Colors.deepPurple,
        brightness: Brightness.dark,
      ),
      home: ValueListenableBuilder(
        valueListenable: connection.state,
        builder: (context, state, _) => switch (state) {
          Starting() || Reconnecting() => const _StatusScreen(
            busy: true,
            message: 'Connecting to your display…',
          ),
          NeedsPairing() || Scanning() || Securing() || PairingFailed() =>
            PairingScreen(connection: connection, state: state),
          Unreachable(:final reason) => _StatusScreen(
            message: "Can't reach your display.\n$reason",
            actions: [
              FilledButton(
                onPressed: connection.start,
                child: const Text('Retry'),
              ),
              TextButton(
                onPressed: connection.choosePairing,
                child: const Text('Pair a display'),
              ),
            ],
          ),
          // Placeholder until the control screen lands (#9).
          Connected() => const _StatusScreen(
            message: 'Connected to your display.',
          ),
        },
      ),
    );
  }
}

class _StatusScreen extends StatelessWidget {
  const _StatusScreen({
    required this.message,
    this.busy = false,
    this.actions = const [],
  });

  final String message;
  final bool busy;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) => Scaffold(
    body: Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (busy) ...[
              const CircularProgressIndicator(),
              const SizedBox(height: 24),
            ],
            Text(message, textAlign: TextAlign.center),
            const SizedBox(height: 24),
            ...actions,
          ],
        ),
      ),
    ),
  );
}
