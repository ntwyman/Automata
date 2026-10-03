import 'dart:async';
import 'dart:collection';

import '../protocol.dart';
import 'ble_central.dart';

/// How long to wait for the reply to most Commands: the display applies
/// them on its next frame, so anything slower means the reply was lost.
const _replyTimeout = Duration(seconds: 5);

/// `WIFI` replies only once the join finishes or hits the firmware's own
/// 15s `JOIN_TIMEOUT` (`wifi.rs`), so wait that out with some slack.
const _wifiReplyTimeout = Duration(seconds: 25);

/// The link went down before the display replied.
class LinkDropped implements Exception {
  const LinkDropped();

  @override
  String toString() => 'Disconnected from the display.';
}

/// Sends Commands over one secured [BleLink] and pairs each with its reply.
///
/// The firmware's Session answers every Command with exactly one reply
/// line, strictly in order, so replies are matched first-in, first-out. A
/// Command that times out keeps its place in that queue, so its late reply
/// is swallowed instead of being taken for the next Command's.
class CommandClient {
  CommandClient(this._link) {
    _replies = _link.replyLines.listen(_onReply);
    _link.onDropped.then((_) => _onDropped());
  }

  final BleLink _link;
  late final StreamSubscription<String> _replies;
  final _waiting = Queue<Completer<String>>();
  bool _dropped = false;

  /// Sends [command] and completes with the display's [Reply] to it.
  /// Throws [LinkDropped], a [TimeoutException], a [FormatException] for a
  /// garbled reply, or whatever the write itself failed with.
  Future<Reply> send(Command command) async {
    if (_dropped) throw const LinkDropped();
    // Queued before writing: the reply can arrive before the write's own
    // acknowledgement does.
    final slot = Completer<String>();
    _waiting.add(slot);
    try {
      await _link.writeLine(encodeCommand(command));
    } catch (_) {
      // Nothing reached the display, so no reply will come for this slot.
      _waiting.remove(slot);
      rethrow;
    }
    final line = await slot.future.timeout(
      command is JoinWifi ? _wifiReplyTimeout : _replyTimeout,
    );
    return parseReply(line);
  }

  /// Stops listening for replies; call once the link is no longer used.
  void close() => _replies.cancel();

  void _onReply(String line) {
    if (_waiting.isEmpty) return;
    _waiting.removeFirst().complete(line);
  }

  void _onDropped() {
    _dropped = true;
    while (_waiting.isNotEmpty) {
      _waiting.removeFirst().completeError(const LinkDropped());
    }
  }
}
