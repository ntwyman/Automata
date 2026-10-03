import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/screens/control_screen.dart';

import '../support/fake_command_link.dart';

void main() {
  late FakeCommandLink link;

  setUp(() => link = FakeCommandLink());

  Future<void> pumpScreen(WidgetTester tester) =>
      tester.pumpWidget(MaterialApp(home: ControlScreen(link: link)));

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
}
