import 'package:flutter_test/flutter_test.dart';
import 'package:pico_w_display_app/tz/tz_rules.dart';

void main() {
  group('tzRuleForZone', () {
    test('maps canonical zones to their tz database rule', () {
      expect(tzRuleForZone('America/Los_Angeles'), 'PST8PDT,M3.2.0,M11.1.0');
      expect(tzRuleForZone('Europe/London'), 'GMT0BST,M3.5.0/1,M10.5.0');
      expect(tzRuleForZone('Asia/Kolkata'), 'IST-5:30');
    });

    test('covers backward links phones may still report', () {
      expect(tzRuleForZone('Asia/Calcutta'), tzRuleForZone('Asia/Kolkata'));
      expect(tzRuleForZone('US/Pacific'), tzRuleForZone('America/Los_Angeles'));
    });

    test('is null for a name the table lacks', () {
      expect(tzRuleForZone('Mars/Olympus_Mons'), isNull);
    });
  });

  group('fixedOffsetRule', () {
    test('writes whole-hour offsets west-positive, as POSIX has them', () {
      expect(fixedOffsetRule(const Duration(hours: 5)), '<+05>-5');
      expect(fixedOffsetRule(const Duration(hours: -3)), '<-03>3');
      expect(fixedOffsetRule(const Duration(hours: 14)), '<+14>-14');
    });

    test('keeps minutes for part-hour offsets', () {
      expect(
        fixedOffsetRule(const Duration(hours: 5, minutes: 30)),
        '<+0530>-5:30',
      );
      expect(
        fixedOffsetRule(const Duration(hours: -9, minutes: -30)),
        '<-0930>9:30',
      );
      expect(
        fixedOffsetRule(const Duration(hours: 12, minutes: 45)),
        '<+1245>-12:45',
      );
    });

    test('names UTC itself +00', () {
      expect(fixedOffsetRule(Duration.zero), '<+00>0');
    });
  });

  group('describeOffset', () {
    test('reads as UTC±hh:mm', () {
      expect(
        describeOffset(const Duration(hours: 5, minutes: 30)),
        'UTC+05:30',
      );
      expect(describeOffset(const Duration(hours: -3)), 'UTC-03:00');
      expect(describeOffset(Duration.zero), 'UTC+00:00');
    });
  });
}
