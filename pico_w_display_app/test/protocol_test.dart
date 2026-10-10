import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/protocol.dart';

void main() {
  group('encodeCommand', () {
    test('writes each Command as the line the firmware parses', () {
      expect(encodeCommand(const SetText('12:34')), 'TEXT 12:34');
      expect(encodeCommand(const ResumeClock()), 'CLOCK');
      expect(encodeCommand(const SetColor(0xff, 0x00, 0xaa)), 'COLOR FF00AA');
      expect(encodeCommand(const SetColor(1, 2, 3)), 'COLOR 010203');
      expect(encodeCommand(const SetBrightness(200)), 'BRIGHTNESS 200');
      expect(
        encodeCommand(const JoinWifi('home', 'pass word')),
        'WIFI home pass word',
      );
      expect(encodeCommand(const FactoryReset()), 'RESET');
      expect(
        encodeCommand(const SetTz('PST8PDT,M3.2.0,M11.1.0')),
        'TZ PST8PDT,M3.2.0,M11.1.0',
      );
      expect(encodeCommand(const QueryTime()), 'TIME');
    });

    test('every encoded Command parses back to itself', () {
      const commands = <Command>[
        SetText('09:05'),
        ResumeClock(),
        SetColor(0x12, 0xab, 0xef),
        SetBrightness(0),
        SetBrightness(255),
        JoinWifi('net', 'a b c'),
        FactoryReset(),
        SetTz('<+0530>-5:30'),
        QueryTime(),
      ];
      for (final command in commands) {
        expect(parseCommand(encodeCommand(command)), command);
      }
    });
  });

  group('parseCommand', () {
    test('accepts command names in any case and trims the line', () {
      expect(parseCommand('  clock \r'), const ResumeClock());
      expect(parseCommand('Text 12:34'), const SetText('12:34'));
      expect(parseCommand('reset'), const FactoryReset());
    });

    test('rejects unknown commands', () {
      expect(
        () => parseCommand('BOGUS'),
        throwsProtocolError('unknown command'),
      );
      expect(() => parseCommand(''), throwsProtocolError('unknown command'));
    });

    test('RESET is bare only, and UNPAIR is gone', () {
      expect(() => parseCommand('RESET now'), throwsProtocolError('bad args'));
      expect(
        () => parseCommand('UNPAIR'),
        throwsProtocolError('unknown command'),
      );
    });

    test('TZ needs a rule, which only the firmware validates', () {
      expect(parseCommand('tz UTC0'), const SetTz('UTC0'));
      expect(() => parseCommand('TZ'), throwsProtocolError('bad args'));
      expect(() => parseCommand('TZ  '), throwsProtocolError('bad args'));
    });

    test('TEXT takes exactly a DD:DD shape', () {
      expect(parseCommand('TEXT 00:59'), const SetText('00:59'));
      expect(() => parseCommand('TEXT 1234'), throwsProtocolError('bad args'));
      expect(
        () => parseCommand('TEXT 12:345'),
        throwsProtocolError('bad args'),
      );
      expect(
        () => parseCommand('TEXT ab:34'),
        throwsProtocolError('unsupported char'),
      );
      expect(
        () => parseCommand('TEXT 12-34'),
        throwsProtocolError('unsupported char'),
      );
    });

    test('COLOR accepts hex digits in either case', () {
      expect(parseCommand('COLOR ff00AA'), const SetColor(0xff, 0x00, 0xaa));
      expect(parseCommand('COLOR AbCdEf'), const SetColor(0xab, 0xcd, 0xef));
      expect(
        () => parseCommand('COLOR zzzzzz'),
        throwsProtocolError('bad args'),
      );
      expect(() => parseCommand('COLOR fff'), throwsProtocolError('bad args'));
    });

    test('BRIGHTNESS takes 0 to 255', () {
      expect(parseCommand('BRIGHTNESS 0'), const SetBrightness(0));
      expect(parseCommand('BRIGHTNESS 255'), const SetBrightness(255));
      expect(
        () => parseCommand('BRIGHTNESS 256'),
        throwsProtocolError('bad args'),
      );
      expect(
        () => parseCommand('BRIGHTNESS -1'),
        throwsProtocolError('bad args'),
      );
      expect(
        () => parseCommand('BRIGHTNESS 0x10'),
        throwsProtocolError('bad args'),
      );
      expect(() => parseCommand('BRIGHTNESS'), throwsProtocolError('bad args'));
    });

    test('WIFI splits the SSID off at the first space only', () {
      expect(
        parseCommand('WIFI home my secret pass'),
        const JoinWifi('home', 'my secret pass'),
      );
      expect(() => parseCommand('WIFI home'), throwsProtocolError('bad args'));
      expect(() => parseCommand('WIFI'), throwsProtocolError('bad args'));
    });

    test('WIFI bounds the SSID to 32 bytes and the password to 63', () {
      final ssid32 = 'a' * 32;
      final password63 = 'p' * 63;
      expect(
        parseCommand('WIFI $ssid32 $password63'),
        JoinWifi(ssid32, password63),
      );
      expect(
        () => parseCommand('WIFI ${'a' * 33} pw'),
        throwsProtocolError('bad args'),
      );
      expect(
        () => parseCommand('WIFI net ${'p' * 64}'),
        throwsProtocolError('bad args'),
      );
    });
  });

  group('JoinWifi', () {
    test('never shows its password when printed', () {
      const command = JoinWifi('home', 'hunter2');
      expect('$command', isNot(contains('hunter2')));
      expect('$command', contains('home'));
    });
  });

  group('validation for the control screen', () {
    test('textArgError accepts DD:DD and explains anything else', () {
      expect(textArgError('12:34'), isNull);
      expect(textArgError('12:3'), isNotNull);
      expect(textArgError('1a:34'), isNotNull);
    });

    test('ssidError rejects what the grammar cannot carry', () {
      expect(ssidError('home'), isNull);
      expect(ssidError(''), isNotNull);
      expect(ssidError('my home'), isNotNull);
      expect(ssidError('a' * 33), isNotNull);
      expect(ssidError('café'), isNotNull);
    });

    test('passwordError rejects what the firmware would alter or refuse', () {
      expect(passwordError('pass word'), isNull);
      expect(passwordError(''), isNotNull);
      expect(passwordError(' leading'), isNotNull);
      expect(passwordError('trailing '), isNotNull);
      expect(passwordError('p' * 64), isNotNull);
      expect(passwordError('naïve'), isNotNull);
    });
  });

  group('parseTimeUtc', () {
    test('takes the UTC timestamp TIME replies with', () {
      expect(
        parseTimeUtc('2026-10-09T18:20:43Z GMT0BST,M3.5.0/1,M10.5.0'),
        DateTime.utc(2026, 10, 9, 18, 20, 43),
      );
    });

    test('anything else is a FormatException', () {
      expect(() => parseTimeUtc('soon UTC0'), throwsFormatException);
    });
  });

  group('parseReply', () {
    test('OK with or without a detail', () {
      expect(parseReply('OK\n'), const ReplyOk());
      expect(parseReply('OK 10.0.0.5\n'), const ReplyOk('10.0.0.5'));
      expect(parseReply('OK'), const ReplyOk());
    });

    test('ERR carries its reason', () {
      expect(
        parseReply('ERR wifi join failed\n'),
        const ReplyErr('wifi join failed'),
      );
      expect(parseReply('ERR bad args\r\n'), const ReplyErr('bad args'));
    });

    test('anything else is a FormatException', () {
      expect(() => parseReply('HELLO'), throwsFormatException);
      expect(() => parseReply('OKAY'), throwsFormatException);
      expect(() => parseReply(''), throwsFormatException);
    });
  });
}

Matcher throwsProtocolError(String reason) =>
    throwsA(isA<ProtocolError>().having((e) => e.reason, 'reason', reason));
