import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/screens/control_screen.dart';

import '../support/fake_command_link.dart';

void main() {
  late FakeCommandLink link;
  late int forgotten;

  setUp(() {
    link = FakeCommandLink();
    forgotten = 0;
  });

  /// With no [zone], the phone's zone never resolves, so no `TZ`/`TIME`
  /// joins the queue ahead of what a test sends.
  Future<void> pumpScreen(WidgetTester tester, {String? zone}) =>
      tester.pumpWidget(
        MaterialApp(
          home: ControlScreen(
            link: link,
            onFactoryReset: () async => forgotten++,
            zoneSource: zone == null
                ? () => Completer<String>().future
                : () async => zone,
          ),
        ),
      );

  /// Replies [line] to the oldest unanswered Command, then lets the
  /// screen catch up: a TZ status lands a few async hops after its reply.
  Future<void> reply(WidgetTester tester, String line) async {
    link.reply('$line\n');
    await tester.pump();
    await tester.pump();
  }

  testWidgets('sends the phone\'s TZ Rule on connect, then TIME', (
    tester,
  ) async {
    await pumpScreen(tester, zone: 'Europe/London');
    await tester.pump();
    expect(link.written, ['TZ GMT0BST,M3.5.0/1,M10.5.0']);

    await reply(tester, 'OK');
    expect(link.written.last, 'TIME');
    await reply(tester, 'ERR not synced');

    expect(find.text('Waiting for network time'), findsOneWidget);
  });

  testWidgets('shows the display\'s time once it has Synced', (tester) async {
    await pumpScreen(tester, zone: 'Europe/London');
    await tester.pump();
    await reply(tester, 'OK');
    await reply(tester, 'OK 2026-10-09T18:20:43Z GMT0BST,M3.5.0/1,M10.5.0');

    expect(find.textContaining('Synced · '), findsOneWidget);
  });

  testWidgets('warns when the phone\'s zone is not in the table', (
    tester,
  ) async {
    await pumpScreen(tester, zone: 'Mars/Olympus_Mons');
    await tester.pump();
    await reply(tester, 'OK');

    expect(
      find.textContaining(
        'Timezone not supported: Mars/Olympus_Mons (using UTC',
      ),
      findsOneWidget,
    );
  });

  testWidgets('TEXT sends the entered text and shows the reply inline', (
    tester,
  ) async {
    await pumpScreen(tester);

    await tester.enterText(
      find.widgetWithText(TextField, 'Show text'),
      '12:34',
    );
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, 'Show'));
    await tester.pump();
    expect(link.written, ['TEXT 12:34']);
    expect(find.text('Sending…'), findsOneWidget);

    link.reply('OK\n');
    await tester.pump();

    expect(find.text('OK'), findsOneWidget);
  });

  testWidgets('the TEXT field filters out what the display cannot draw', (
    tester,
  ) async {
    await pumpScreen(tester);

    await tester.enterText(
      find.widgetWithText(TextField, 'Show text'),
      'ab12:345',
    );
    await tester.pump();

    expect(
      tester
          .widget<TextField>(find.widgetWithText(TextField, 'Show text'))
          .controller!
          .text,
      '12:34',
    );
  });

  testWidgets('Resume clock sends a bare CLOCK', (tester) async {
    await pumpScreen(tester);

    await tester.tap(find.text('Resume clock'));
    await tester.pump();

    expect(link.written, ['CLOCK']);
    link.reply('OK\n');
    await tester.pump();
  });

  testWidgets('WIFI shows ERR inline and clears the masked password', (
    tester,
  ) async {
    await pumpScreen(tester);
    final password = find.widgetWithText(TextFormField, 'Password');
    await tester.scrollUntilVisible(
      find.text('Join network'),
      200,
      scrollable: find.byType(Scrollable).first,
    );

    await tester.enterText(
      find.widgetWithText(TextFormField, 'Network name (SSID)'),
      'home',
    );
    await tester.enterText(password, 'wrong pass');
    expect(
      tester
          .widget<TextField>(
            find.descendant(of: password, matching: find.byType(TextField)),
          )
          .obscureText,
      isTrue,
    );
    await tester.tap(find.text('Join network'));
    await tester.pump();
    expect(link.written, ['WIFI home wrong pass']);

    link.reply('ERR wifi join failed\n');
    await tester.pump();

    expect(find.text('ERR wifi join failed'), findsOneWidget);
    expect(find.text('wrong pass'), findsNothing);
  });

  /// Scrolls to the Factory Reset button and taps it.
  Future<void> tapFactoryReset(WidgetTester tester) async {
    final button = find.widgetWithText(OutlinedButton, 'Factory reset');
    await tester.scrollUntilVisible(
      button,
      200,
      scrollable: find.byType(Scrollable).first,
    );
    // Built, but possibly still under the bottom edge.
    await tester.ensureVisible(button);
    await tester.pumpAndSettle();
    await tester.tap(button);
    await tester.pumpAndSettle();
  }

  /// Confirms the dialog. Not `pumpAndSettle`: the Sending… spinner never
  /// settles, so it would run the clock out to the reply timeout.
  Future<void> confirmReset(WidgetTester tester) async {
    await tester.tap(find.widgetWithText(FilledButton, 'Reset'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
  }

  testWidgets('Factory reset asks first, and cancelling sends nothing', (
    tester,
  ) async {
    await pumpScreen(tester);

    await tapFactoryReset(tester);
    expect(find.byType(AlertDialog), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();

    expect(link.written, isEmpty);
    expect(forgotten, 0);
  });

  testWidgets('a confirmed Factory reset sends RESET and forgets on OK', (
    tester,
  ) async {
    await pumpScreen(tester);

    await tapFactoryReset(tester);
    await confirmReset(tester);
    expect(link.written, ['RESET']);
    expect(forgotten, 0);

    await reply(tester, 'OK');
    await tester.pumpAndSettle();

    expect(forgotten, 1);
  });

  testWidgets('a Factory reset the display refuses forgets nothing', (
    tester,
  ) async {
    await pumpScreen(tester);

    await tapFactoryReset(tester);
    await confirmReset(tester);
    await reply(tester, 'ERR unknown command');
    await tester.pumpAndSettle();

    expect(forgotten, 0);
    expect(find.text('ERR unknown command'), findsOneWidget);
  });
}
