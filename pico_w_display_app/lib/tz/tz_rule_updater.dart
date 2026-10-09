import 'package:flutter/foundation.dart';
import 'package:flutter_timezone/flutter_timezone.dart';

import '../ble/command_client.dart';
import '../protocol.dart';
import 'tz_rules.dart';

/// The phone's IANA zone name, e.g. `America/Los_Angeles`.
typedef ZoneSource = Future<String> Function();

/// [ZoneSource] for a real phone.
Future<String> platformZone() async =>
    (await FlutterTimezone.getLocalTimezone()).identifier;

/// What [TzRuleUpdater] last learned about the display's Wall Clock.
sealed class TzStatus {
  const TzStatus();
}

/// `TIME` replied: the display has Synced and shows local time.
class TzSynced extends TzStatus {
  const TzSynced(this.utc);

  /// The display's UTC as of its reply.
  final DateTime utc;

  @override
  bool operator ==(Object other) => other is TzSynced && other.utc == utc;

  @override
  int get hashCode => utc.hashCode;

  @override
  String toString() => 'TzSynced($utc)';
}

/// The display is Unsynced: no network time yet.
class TzUnsynced extends TzStatus {
  const TzUnsynced();

  @override
  bool operator ==(Object other) => other is TzUnsynced;

  @override
  int get hashCode => (TzUnsynced).hashCode;

  @override
  String toString() => 'TzUnsynced()';
}

/// The phone's [zone] isn't in the table, so the display was sent a
/// [fixedOffsetRule] for [offset] instead, which won't follow DST.
class TzUnsupported extends TzStatus {
  const TzUnsupported(this.zone, this.offset);

  final String zone;
  final Duration offset;

  @override
  bool operator ==(Object other) =>
      other is TzUnsupported && other.zone == zone && other.offset == offset;

  @override
  int get hashCode => Object.hash(zone, offset);

  @override
  String toString() => 'TzUnsupported($zone, $offset)';
}

/// The display answered `TZ` with `ERR`: most likely a table rule the
/// firmware can't parse, which `tz.rs`'s fixture test should have caught.
class TzRejected extends TzStatus {
  const TzRejected();

  @override
  bool operator ==(Object other) => other is TzRejected;

  @override
  int get hashCode => (TzRejected).hashCode;

  @override
  String toString() => 'TzRejected()';
}

/// Keeps one link's display on the phone's TZ Rule: sends `TZ` then `TIME`
/// on [start], and again on [resumed] (`TZ` only if the rule has changed,
/// e.g. after travel or, for a [fixedOffsetRule], a DST change). Never
/// blocks the controls: its Commands just queue in the shared
/// [CommandClient] alongside theirs.
///
/// [status] shows the most pressing of: `TZ` rejected, then the zone being
/// unsupported (no `TIME` is sent then: the warning stands either way), then
/// `TIME`'s answer. A `TZ` or `TIME` with no reply leaves it `null`, as it is
/// while in flight; the `TZ` is retried on the next [resumed] or connect.
class TzRuleUpdater {
  TzRuleUpdater(
    this._client, {
    this._zoneSource = platformZone,
    Duration Function()? offsetNow,
  }) : _offsetNow = offsetNow ?? (() => DateTime.now().timeZoneOffset);

  final CommandClient _client;
  final ZoneSource _zoneSource;
  final Duration Function() _offsetNow;

  /// What the last update learned, for the control screen's status line.
  final status = ValueNotifier<TzStatus?>(null);

  /// The rule the display last accepted over this link.
  String? _sentRule;

  /// Updates are chained so a resume during the first can't interleave
  /// with it.
  Future<void> _running = Future.value();
  bool _disposed = false;

  /// Updates the display once its link is up.
  void start() => _enqueue();

  /// Updates the display again, e.g. when the app returns to the foreground.
  void resumed() => _enqueue();

  /// Stops updating; one already in flight finishes without reporting.
  void dispose() {
    _disposed = true;
    status.dispose();
  }

  void _enqueue() => _running = _running.then((_) => _update());

  Future<void> _update() async {
    if (_disposed) return;
    final TzStatus? next;
    try {
      next = await _updateOnce();
    } catch (_) {
      // No reply (timeout, dropped link, garbled line): nothing to show.
      _show(null);
      return;
    }
    _show(next);
  }

  Future<TzStatus?> _updateOnce() async {
    String? zone;
    try {
      zone = await _zoneSource();
    } catch (_) {
      // Treated like a zone the table lacks.
    }
    final known = zone == null ? null : tzRuleForZone(zone);
    final offset = _offsetNow();
    final rule = known ?? fixedOffsetRule(offset);

    if (rule != _sentRule) {
      if (await _client.send(SetTz(rule)) is ReplyErr) {
        return const TzRejected();
      }
      _sentRule = rule;
    }
    if (known == null) return TzUnsupported(zone ?? 'unknown', offset);

    return switch (await _client.send(const QueryTime())) {
      ReplyOk(:final detail?) => TzSynced(parseTimeUtc(detail)),
      ReplyErr(reason: 'not synced') => const TzUnsynced(),
      _ => null,
    };
  }

  void _show(TzStatus? next) {
    if (!_disposed) status.value = next;
  }
}
