import 'dart:async';

import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/ble/command_client.dart';
import 'package:pico_w_display_app/protocol.dart';

import '../support/fake_command_link.dart';

void main() {
  late FakeCommandLink link;
  late CommandClient client;

  setUp(() {
    link = FakeCommandLink();
    client = CommandClient(link);
  });

  tearDown(() => client.close());

  test('writes the encoded Command and returns the reply to it', () async {
    final reply = client.send(const SetBrightness(200));
    await pumpEventQueue();
    expect(link.written, ['BRIGHTNESS 200']);

    link.reply('OK\n');

    expect(await reply, const ReplyOk());
  });

  test('surfaces ERR with its reason', () async {
    final reply = client.send(const JoinWifi('home', 'wrong'));
    await pumpEventQueue();

    link.reply('ERR wifi join failed\n');

    expect(await reply, const ReplyErr('wifi join failed'));
  });

  test('matches replies to Commands in the order they were sent', () async {
    final first = client.send(const ResumeClock());
    final second = client.send(const JoinWifi('home', 'pw'));
    await pumpEventQueue();
    expect(link.written, ['CLOCK', 'WIFI home pw']);

    link.reply('OK\n');
    link.reply('OK 10.0.0.5\n');

    expect(await first, const ReplyOk());
    expect(await second, const ReplyOk('10.0.0.5'));
  });

  test(
    'a reply that already arrived before the write completed still counts',
    () async {
      link.replyDuringWrite = 'OK\n';

      expect(await client.send(const ResumeClock()), const ReplyOk());
    },
  );

  test(
    'a write that fails throws and leaves the next Command unaffected',
    () async {
      link.failNextWrite = true;
      await expectLater(
        client.send(const ResumeClock()),
        throwsA(isA<Exception>()),
      );

      final reply = client.send(const SetBrightness(1));
      await pumpEventQueue();
      link.reply('OK\n');

      expect(await reply, const ReplyOk());
    },
  );

  test('a garbled reply fails only its own Command', () async {
    final reply = client.send(const ResumeClock());
    await pumpEventQueue();

    link.reply('HELLO\n');

    await expectLater(reply, throwsFormatException);
  });

  test('a reply that never comes times out', () {
    fakeAsync((async) {
      Object? error;
      client.send(const ResumeClock()).catchError((Object e) {
        error = e;
        return const ReplyOk();
      });

      async.elapse(const Duration(seconds: 4));
      expect(error, isNull);
      async.elapse(const Duration(seconds: 2));
      expect(error, isA<TimeoutException>());
    });
  });

  test('WIFI waits out the firmware\'s join timeout before giving up', () {
    fakeAsync((async) {
      Object? error;
      client.send(const JoinWifi('home', 'pw')).catchError((Object e) {
        error = e;
        return const ReplyOk();
      });

      async.elapse(const Duration(seconds: 16));
      expect(error, isNull);
      async.elapse(const Duration(seconds: 10));
      expect(error, isA<TimeoutException>());
    });
  });

  test('a late reply to a timed-out Command is not taken for the next one', () {
    fakeAsync((async) {
      client
          .send(const ResumeClock())
          .catchError((Object _) => const ReplyOk());
      async.elapse(const Duration(seconds: 6));

      Reply? second;
      client.send(const SetBrightness(5)).then((r) => second = r);
      async.flushMicrotasks();
      link.reply('ERR late\n');
      async.flushMicrotasks();
      expect(second, isNull);

      link.reply('OK\n');
      async.flushMicrotasks();
      expect(second, const ReplyOk());
    });
  });

  test('a dropped link fails every Command still waiting', () async {
    final reply = client.send(const ResumeClock());
    await pumpEventQueue();

    link.drop();

    await expectLater(reply, throwsA(isA<LinkDropped>()));
  });

  test('sending after the link dropped fails without writing', () async {
    link.drop();
    await pumpEventQueue();

    await expectLater(
      client.send(const ResumeClock()),
      throwsA(isA<LinkDropped>()),
    );
    expect(link.written, isEmpty);
  });
}
