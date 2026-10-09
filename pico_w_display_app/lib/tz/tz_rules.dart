/// Turning the phone's timezone into a TZ Rule the display can apply: the
/// generated IANA → POSIX table, and a fixed-offset fallback for zones it
/// lacks. Per `pico_w_display/docs/adr/0003-posix-tz-rule-on-device.md`,
/// this mapping is the app's job; the device only ever sees POSIX.
library;

import 'posix_table.g.dart';

/// The tz database's own POSIX rule for IANA [zone], or `null` if the
/// table (built from tzdata [tzdataVersion]) doesn't have it.
String? tzRuleForZone(String zone) => posixRules[zone];

/// A rule holding [offset] east of UTC year-round, named the way the tz
/// database names unnamed zones (`<+0530>`). Only right until the next DST
/// change, so it's a stopgap for a zone missing from the table.
String fixedOffsetRule(Duration offset) {
  final (sign, hours, minutes) = _parts(offset);
  final name = minutes == 0
      ? '$sign${_two(hours)}'
      : '$sign${_two(hours)}${_two(minutes)}';
  // POSIX offsets count hours *west* of UTC, so the sign flips.
  final west = offset.isNegative || offset == Duration.zero ? '' : '-';
  final value = minutes == 0 ? '$hours' : '$hours:${_two(minutes)}';
  return '<$name>$west$value';
}

/// [offset] east of UTC as `UTC±hh:mm`, for showing alongside a
/// [fixedOffsetRule].
String describeOffset(Duration offset) {
  final (sign, hours, minutes) = _parts(offset);
  return 'UTC$sign${_two(hours)}:${_two(minutes)}';
}

/// [offset]'s sign and magnitude, to the nearest minute.
(String, int, int) _parts(Duration offset) {
  final total = (offset.inSeconds.abs() / 60).round();
  return (offset.isNegative ? '-' : '+', total ~/ 60, total % 60);
}

String _two(int n) => '$n'.padLeft(2, '0');
