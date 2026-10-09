/// The display's line-based Command grammar, mirroring the firmware's
/// `pico_w_display/src/protocol.rs` (`parse_line` and friends) so the app
/// can build and check lines without a device. Pure functions only: the BLE
/// side lives in `ble/`.
///
/// One Command per line; every Command gets exactly one reply line, `OK`
/// (optionally followed by a detail, e.g. `WIFI`'s address) or
/// `ERR <reason>`. Lines here carry no terminator: one GATT write is one
/// line, and the firmware appends the `\n` itself.
library;

/// A Command the display understands; see [encodeCommand] and
/// [parseCommand].
sealed class Command {
  const Command();
}

/// `TEXT DD:DD`: shows [text] in place of the clock until [ResumeClock].
class SetText extends Command {
  const SetText(this.text);

  final String text;

  @override
  bool operator ==(Object other) => other is SetText && other.text == text;

  @override
  int get hashCode => text.hashCode;

  @override
  String toString() => 'SetText($text)';
}

/// Bare `CLOCK`: goes back to the clock.
class ResumeClock extends Command {
  const ResumeClock();

  @override
  bool operator ==(Object other) => other is ResumeClock;

  @override
  int get hashCode => (ResumeClock).hashCode;

  @override
  String toString() => 'ResumeClock()';
}

/// `COLOR RRGGBB`: the color glyphs are drawn in, each channel 0–255.
class SetColor extends Command {
  const SetColor(this.red, this.green, this.blue);

  final int red;
  final int green;
  final int blue;

  @override
  bool operator ==(Object other) =>
      other is SetColor &&
      other.red == red &&
      other.green == green &&
      other.blue == blue;

  @override
  int get hashCode => Object.hash(red, green, blue);

  @override
  String toString() => 'SetColor($red, $green, $blue)';
}

/// `BRIGHTNESS N`: overall LED brightness, 0–255.
class SetBrightness extends Command {
  const SetBrightness(this.level);

  final int level;

  @override
  bool operator ==(Object other) =>
      other is SetBrightness && other.level == level;

  @override
  int get hashCode => level.hashCode;

  @override
  String toString() => 'SetBrightness($level)';
}

/// `WIFI <ssid> <password>`: joins a network; replies `OK <address>`.
class JoinWifi extends Command {
  const JoinWifi(this.ssid, this.password);

  final String ssid;
  final String password;

  @override
  bool operator ==(Object other) =>
      other is JoinWifi && other.ssid == ssid && other.password == password;

  @override
  int get hashCode => Object.hash(ssid, password);

  /// Never includes [password], so a stray print or error message can't
  /// leak it.
  @override
  String toString() => 'JoinWifi($ssid, <redacted>)';
}

/// Bare `UNPAIR`: clears the device's persisted Bond.
class Unpair extends Command {
  const Unpair();

  @override
  bool operator ==(Object other) => other is Unpair;

  @override
  int get hashCode => (Unpair).hashCode;

  @override
  String toString() => 'Unpair()';
}

/// `TZ <posix>`: persists and applies a TZ Rule. Only the firmware checks
/// [rule]'s grammar, replying `ERR bad tz` if it can't use it.
class SetTz extends Command {
  const SetTz(this.rule);

  final String rule;

  @override
  bool operator ==(Object other) => other is SetTz && other.rule == rule;

  @override
  int get hashCode => rule.hashCode;

  @override
  String toString() => 'SetTz($rule)';
}

/// Bare `TIME`: replies `OK <UTC ISO-8601> <TZ Rule>`, or `ERR not synced`
/// before the device's first Sync.
class QueryTime extends Command {
  const QueryTime();

  @override
  bool operator ==(Object other) => other is QueryTime;

  @override
  int get hashCode => (QueryTime).hashCode;

  @override
  String toString() => 'QueryTime()';
}

/// The line the firmware parses back into [command]. Assumes [command]'s
/// fields are already in range (see the `*Error` validators below).
String encodeCommand(Command command) => switch (command) {
  SetText(:final text) => 'TEXT $text',
  ResumeClock() => 'CLOCK',
  SetColor(:final red, :final green, :final blue) =>
    'COLOR ${_hex(red)}${_hex(green)}${_hex(blue)}',
  SetBrightness(:final level) => 'BRIGHTNESS $level',
  JoinWifi(:final ssid, :final password) => 'WIFI $ssid $password',
  Unpair() => 'UNPAIR',
  SetTz(:final rule) => 'TZ $rule',
  QueryTime() => 'TIME',
};

String _hex(int channel) =>
    channel.toRadixString(16).padLeft(2, '0').toUpperCase();

/// A line the firmware would reject, carrying the same reason it would
/// reply `ERR` with.
class ProtocolError implements Exception {
  const ProtocolError(this.reason);

  static const unknownCommand = ProtocolError('unknown command');
  static const badArgs = ProtocolError('bad args');
  static const unsupportedChar = ProtocolError('unsupported char');

  final String reason;

  @override
  String toString() => 'ProtocolError: $reason';
}

/// Parses one Command line exactly as `protocol.rs`'s `parse_line` does:
/// command names in any case, and `WIFI`'s password is everything after the
/// SSID's first space. Throws [ProtocolError] otherwise.
Command parseCommand(String line) {
  final (name, rest) = _splitFirstSpace(line.trim());
  return switch (name.toUpperCase()) {
    'TEXT' => _parseText(rest.trim()),
    'CLOCK' => const ResumeClock(),
    'COLOR' => _parseColor(rest.trim()),
    'BRIGHTNESS' => _parseBrightness(rest.trim()),
    'WIFI' => _parseWifi(rest.trim()),
    'UNPAIR' => const Unpair(),
    'TZ' => _parseTz(rest.trim()),
    'TIME' => const QueryTime(),
    _ => throw ProtocolError.unknownCommand,
  };
}

(String, String) _splitFirstSpace(String s) {
  final i = s.indexOf(' ');
  return i < 0 ? (s, '') : (s.substring(0, i), s.substring(i + 1));
}

Command _parseText(String s) {
  if (s.length != 5) throw ProtocolError.badArgs;
  if (!_textShape.hasMatch(s)) throw ProtocolError.unsupportedChar;
  return SetText(s);
}

final _textShape = RegExp(r'^[0-9]{2}:[0-9]{2}$');

// Stricter than the firmware in one corner: Rust's `u8::from_str_radix`
// also takes a sign per byte pair (`+F+F+F`), which nothing should send.
Command _parseColor(String s) {
  if (!RegExp(r'^[0-9a-fA-F]{6}$').hasMatch(s)) throw ProtocolError.badArgs;
  int channel(int at) => int.parse(s.substring(at, at + 2), radix: 16);
  return SetColor(channel(0), channel(2), channel(4));
}

Command _parseBrightness(String s) {
  // Not `int.tryParse`: that also takes `-1` and `0x10`, which Rust's
  // `u16::from_str` doesn't.
  if (!RegExp(r'^\+?[0-9]+$').hasMatch(s)) throw ProtocolError.badArgs;
  final level = int.parse(s);
  if (level > 255) throw ProtocolError.badArgs;
  return SetBrightness(level);
}

Command _parseWifi(String s) {
  final (ssid, rest) = _splitFirstSpace(s);
  final password = rest.trim();
  if (ssid.isEmpty || password.isEmpty) throw ProtocolError.badArgs;
  if (ssid.length > maxSsidLength || password.length > maxPasswordLength) {
    throw ProtocolError.badArgs;
  }
  return JoinWifi(ssid, password);
}

Command _parseTz(String s) {
  if (s.isEmpty) throw ProtocolError.badArgs;
  return SetTz(s);
}

/// 802.11's SSID limit, and `parse_wifi`'s `String<32>`.
const maxSsidLength = 32;

/// WPA2/3's passphrase limit, and `parse_wifi`'s `String<63>`.
const maxPasswordLength = 63;

// The firmware reads a line byte by byte as Latin-1 (`read_line`), so
// anything outside printable ASCII wouldn't reach it intact. The validators
// below hold user input to that, and to what the grammar can carry, before
// it's ever encoded.
final _printableAscii = RegExp(r'^[\x20-\x7e]*$');

/// Why [text] can't be a `TEXT` argument, or `null` if it can.
String? textArgError(String text) =>
    _textShape.hasMatch(text) ? null : 'Use the form 12:34 — digits only.';

/// Why [ssid] can't be sent in a `WIFI` Command, or `null` if it can.
String? ssidError(String ssid) {
  if (ssid.isEmpty) return 'Enter the network name.';
  if (!_printableAscii.hasMatch(ssid)) {
    return 'Only plain ASCII characters are supported.';
  }
  if (ssid.contains(' ')) return 'Network names with spaces are not supported.';
  if (ssid.length > maxSsidLength) {
    return 'At most $maxSsidLength characters.';
  }
  return null;
}

/// Why [password] can't be sent in a `WIFI` Command, or `null` if it can.
/// Inner spaces are fine; leading/trailing ones would be trimmed off by the
/// firmware, so they're refused rather than silently changed.
String? passwordError(String password) {
  if (password.isEmpty) return 'Enter the password.';
  if (!_printableAscii.hasMatch(password)) {
    return 'Only plain ASCII characters are supported.';
  }
  if (password.trim() != password) {
    return 'The password cannot start or end with a space.';
  }
  if (password.length > maxPasswordLength) {
    return 'At most $maxPasswordLength characters.';
  }
  return null;
}

/// The display's answer to one Command.
sealed class Reply {
  const Reply();
}

/// `OK`, with whatever followed it (`WIFI`'s address), if anything.
class ReplyOk extends Reply {
  const ReplyOk([this.detail]);

  final String? detail;

  @override
  bool operator ==(Object other) => other is ReplyOk && other.detail == detail;

  @override
  int get hashCode => detail.hashCode;

  @override
  String toString() => detail == null ? 'OK' : 'OK $detail';
}

/// `ERR <reason>`.
class ReplyErr extends Reply {
  const ReplyErr(this.reason);

  final String reason;

  @override
  bool operator ==(Object other) => other is ReplyErr && other.reason == reason;

  @override
  int get hashCode => reason.hashCode;

  @override
  String toString() => 'ERR $reason';
}

/// The UTC in `TIME`'s `OK <UTC ISO-8601> <TZ Rule>` [detail]. Throws
/// [FormatException] if it doesn't start with a timestamp.
DateTime parseTimeUtc(String detail) => DateTime.parse(detail.split(' ').first);

/// Parses one reply line (terminator optional). Throws [FormatException]
/// for anything that isn't `OK…` or `ERR …`.
Reply parseReply(String line) {
  final (status, rest) = _splitFirstSpace(line.trim());
  final detail = rest.trim();
  return switch (status) {
    'OK' => ReplyOk(detail.isEmpty ? null : detail),
    'ERR' => ReplyErr(detail),
    _ => throw FormatException('Unexpected reply', line),
  };
}
