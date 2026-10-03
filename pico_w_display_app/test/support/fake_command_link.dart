import 'dart:async';

import 'package:pico_w_display_app/ble/ble_central.dart';

/// A [BleLink] that records writes and lets the test push reply lines.
class FakeCommandLink implements BleLink {
  final written = <String>[];
  final _replies = StreamController<String>.broadcast(sync: true);
  final _dropped = Completer<void>();

  bool failNextWrite = false;
  String? replyDuringWrite;

  void reply(String line) => _replies.add(line);

  void drop() => _dropped.complete();

  @override
  Future<void> writeLine(String line) async {
    if (failNextWrite) {
      failNextWrite = false;
      throw Exception('write failed');
    }
    written.add(line);
    if (replyDuringWrite case final line?) {
      replyDuringWrite = null;
      reply(line);
    }
  }

  @override
  Stream<String> get replyLines => _replies.stream;

  @override
  Future<void> get onDropped => _dropped.future;

  @override
  Future<void> secure({required bool pairing}) async {}

  @override
  Future<void> disconnect() async => drop();
}
