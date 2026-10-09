import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/ble/command_client.dart';
import 'package:pico_w_display_app/tz/tz_rule_updater.dart';

import '../support/fake_command_link.dart';

void main() {
  late FakeCommandLink link;
  late CommandClient client;
  late String zone;
  late Duration offset;
  late TzRuleUpdater updater;

  setUp(() {
    link = FakeCommandLink();
    client = CommandClient(link);
    zone = 'America/Los_Angeles';
    offset = const Duration(hours: -7);
    updater = TzRuleUpdater(
      client,
      zoneSource: () async => zone,
      offsetNow: () => offset,
    );
  });

  tearDown(() {
    updater.dispose();
    client.close();
  });

  /// Answers the Commands [updater] has written so far, one reply each, in
  /// order, letting it write the next in between.
  Future<void> answer(List<String> replies) async {
    for (final reply in replies) {
      await pumpEventQueue();
      link.reply('$reply\n');
    }
    await pumpEventQueue();
  }

  test('sends the zone\'s TZ Rule, then TIME, on start', () async {
    updater.start();
    await answer(['OK', 'OK 2026-10-09T18:20:43Z PST8PDT,M3.2.0,M11.1.0']);

    expect(link.written, ['TZ PST8PDT,M3.2.0,M11.1.0', 'TIME']);
    expect(
      updater.status.value,
      TzSynced(DateTime.utc(2026, 10, 9, 18, 20, 43)),
    );
  });

  test('shows the display is Unsynced', () async {
    updater.start();
    await answer(['OK', 'ERR not synced']);

    expect(updater.status.value, const TzUnsynced());
  });

  test(
    'falls back to the phone\'s current offset for an unknown zone',
    () async {
      zone = 'Mars/Olympus_Mons';
      offset = const Duration(hours: 5, minutes: 30);

      updater.start();
      await answer(['OK']);

      expect(link.written, ['TZ <+0530>-5:30']);
      expect(
        updater.status.value,
        const TzUnsupported(
          'Mars/Olympus_Mons',
          Duration(hours: 5, minutes: 30),
        ),
      );
    },
  );

  test('falls back too when the platform can\'t say the zone', () async {
    updater = TzRuleUpdater(
      client,
      zoneSource: () async => throw Exception('no zone'),
      offsetNow: () => Duration.zero,
    );

    updater.start();
    await answer(['OK']);

    expect(link.written.first, 'TZ <+00>0');
    expect(updater.status.value, isA<TzUnsupported>());
  });

  test('a rule the display rejects is shown, and TIME is skipped', () async {
    updater.start();
    await answer(['ERR bad tz']);

    expect(link.written, ['TZ PST8PDT,M3.2.0,M11.1.0']);
    expect(updater.status.value, const TzRejected());
  });

  group('on resume', () {
    setUp(() async {
      updater.start();
      await answer(['OK', 'ERR not synced']);
      link.written.clear();
    });

    test('only re-runs TIME when the zone is unchanged', () async {
      updater.resumed();
      await answer(['OK 2026-10-09T18:20:43Z PST8PDT,M3.2.0,M11.1.0']);

      expect(link.written, ['TIME']);
      expect(updater.status.value, isA<TzSynced>());
    });

    test('re-sends TZ once the zone has changed', () async {
      zone = 'Europe/London';

      updater.resumed();
      await answer(['OK', 'OK 2026-10-09T18:20:43Z GMT0BST,M3.5.0/1,M10.5.0']);

      expect(link.written, ['TZ GMT0BST,M3.5.0/1,M10.5.0', 'TIME']);
    });

    test('re-sends a fallback rule once the offset has changed', () async {
      zone = 'Mars/Olympus_Mons';
      updater.resumed();
      await answer(['OK']);
      link.written.clear();

      offset = const Duration(hours: -8);
      updater.resumed();
      await answer(['OK']);

      expect(link.written, ['TZ <-08>8']);
    });
  });

  test('a TZ that never got its reply is re-sent on resume', () async {
    updater.start();
    await pumpEventQueue();
    link.reply('garbled\n');
    await pumpEventQueue();
    expect(updater.status.value, isNull);
    link.written.clear();

    updater.resumed();
    await answer(['OK', 'ERR not synced']);

    expect(link.written, ['TZ PST8PDT,M3.2.0,M11.1.0', 'TIME']);
  });

  test('stays silent when TIME gets no reply in time', () {
    fakeAsync((async) {
      // Built inside the fake zone so its timeouts run on fake time.
      final link = FakeCommandLink();
      final client = CommandClient(link);
      final updater = TzRuleUpdater(client, zoneSource: () async => zone);

      updater.start();
      async.flushMicrotasks();
      link.reply('OK\n');
      async.elapse(const Duration(seconds: 10));

      expect(link.written.last, 'TIME');
      expect(updater.status.value, isNull);
      updater.dispose();
      client.close();
    });
  });

  test('stays silent when the link drops', () async {
    updater.start();
    await pumpEventQueue();

    link.drop();
    await pumpEventQueue();

    expect(updater.status.value, isNull);
  });
}
