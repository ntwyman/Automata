import 'package:flutter/material.dart';

import 'ble/device_connection.dart';
import 'ble/flutter_blue_central.dart';
import 'ble/shared_prefs_known_device_store.dart';
import 'screens/control_screen.dart';
import 'screens/link_banner.dart';
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
      builder: (context, screen) => ValueListenableBuilder(
        valueListenable: connection.state,
        builder: (context, state, _) => Column(
          children: [
            LinkBanner(state: state),
            Expanded(
              // The banner already sits under the status bar.
              child: MediaQuery.removePadding(
                context: context,
                removeTop: true,
                child: screen!,
              ),
            ),
          ],
        ),
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
          // Keyed by link so a reconnect starts the screen afresh on the
          // new link rather than reusing the dropped one's CommandClient.
          Connected(:final link) => ControlScreen(
            key: ObjectKey(link),
            link: link,
            onFactoryReset: connection.forgetDevice,
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
