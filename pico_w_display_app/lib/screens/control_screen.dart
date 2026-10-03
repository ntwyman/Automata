import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../ble/ble_central.dart';
import '../ble/command_client.dart';
import '../protocol.dart';

/// The display's boot-time foreground color and brightness (`main.rs`'s
/// `DARK_BLUE`, `grid.rs`'s full brightness). The display can't be asked
/// what it's showing, so the controls start from these.
const _bootColor = Color(0xff00008b);
const _bootBrightness = 255.0;

/// Day-to-day control over a Connected display: one control per Command,
/// each showing the display's own `OK`/`ERR` reply to it inline.
class ControlScreen extends StatefulWidget {
  const ControlScreen({super.key, required this.link});

  final BleLink link;

  @override
  State<ControlScreen> createState() => _ControlScreenState();
}

class _ControlScreenState extends State<ControlScreen> {
  late final CommandClient _client = CommandClient(widget.link);

  late final _textSender = _Sender(_client);
  late final _clockSender = _Sender(_client);
  late final _colorSender = _Sender(_client);
  late final _brightnessSender = _Sender(_client);
  late final _wifiSender = _Sender(_client);

  @override
  void dispose() {
    for (final sender in [
      _textSender,
      _clockSender,
      _colorSender,
      _brightnessSender,
      _wifiSender,
    ]) {
      sender.dispose();
    }
    _client.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Plasma Display')),
    body: ListView(
      padding: const EdgeInsets.all(16),
      children: [
        _Section(
          title: 'Message',
          children: [
            _TextControl(sender: _textSender),
            const SizedBox(height: 8),
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton.icon(
                onPressed: () => _clockSender.send(const ResumeClock()),
                icon: const Icon(Icons.schedule),
                label: const Text('Resume clock'),
              ),
            ),
            _OutcomeLine(_clockSender),
          ],
        ),
        _Section(
          title: 'Color',
          children: [_ColorControl(sender: _colorSender)],
        ),
        _Section(
          title: 'Brightness',
          children: [_BrightnessControl(sender: _brightnessSender)],
        ),
        _Section(
          title: 'Wi-Fi',
          children: [_WifiControl(sender: _wifiSender)],
        ),
      ],
    ),
  );
}

/// Where one control's latest Command stands.
sealed class _Outcome {
  const _Outcome();
}

class _Sending extends _Outcome {
  const _Sending();
}

class _Replied extends _Outcome {
  const _Replied(this.reply);

  final Reply reply;
}

/// No reply to show: the write failed, timed out, or the link dropped.
class _Failed extends _Outcome {
  const _Failed(this.message);

  final String message;
}

/// One control's channel to the display: sends its Commands and keeps the
/// latest one's [_Outcome] for [_OutcomeLine] to show.
class _Sender {
  _Sender(this._client);

  final CommandClient _client;
  final outcome = ValueNotifier<_Outcome?>(null);

  bool get busy => outcome.value is _Sending;

  Future<void> send(Command command) async {
    outcome.value = const _Sending();
    try {
      outcome.value = _Replied(await _client.send(command));
    } catch (e) {
      outcome.value = _Failed(_describe(e));
    }
  }

  void dispose() => outcome.dispose();

  // Deliberately generic for anything unexpected: never interpolates the
  // Command, which may carry a Wi-Fi password.
  static String _describe(Object e) => switch (e) {
    TimeoutException() => 'No reply from the display.',
    LinkDropped() => '$e',
    FormatException() => 'The display sent an unexpected reply.',
    _ => 'Could not send: ${'$e'.replaceFirst(RegExp(r'^Exception: '), '')}',
  };
}

/// [sender]'s latest outcome, as one line under its control: the display's
/// reply verbatim (`OK …` / `ERR <reason>`), or why there wasn't one.
class _OutcomeLine extends StatelessWidget {
  const _OutcomeLine(this.sender);

  final _Sender sender;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: sender.outcome,
    builder: (context, outcome, _) {
      if (outcome == null) return const SizedBox.shrink();
      final colors = Theme.of(context).colorScheme;
      final (Widget icon, String label, Color? color) = switch (outcome) {
        _Sending() => (
          const SizedBox.square(
            dimension: 16,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
          'Sending…',
          null,
        ),
        _Replied(reply: final ReplyOk ok) => (
          Icon(Icons.check_circle, size: 16, color: colors.primary),
          '$ok',
          null,
        ),
        _Replied(reply: final ReplyErr err) => (
          Icon(Icons.error, size: 16, color: colors.error),
          '$err',
          colors.error,
        ),
        _Failed(:final message) => (
          Icon(Icons.error_outline, size: 16, color: colors.error),
          message,
          colors.error,
        ),
      };
      return Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Row(
          children: [
            icon,
            const SizedBox(width: 8),
            Expanded(
              child: Text(label, style: TextStyle(color: color)),
            ),
          ],
        ),
      );
    },
  );
}

/// A control's send button, disabled while [sender]'s last Command is
/// still waiting on its reply.
class _SendButton extends StatelessWidget {
  const _SendButton({
    required this.sender,
    required this.label,
    required this.onPressed,
  });

  final _Sender sender;
  final String label;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: sender.outcome,
    builder: (context, _, _) => FilledButton(
      onPressed: sender.busy ? null : onPressed,
      child: Text(label),
    ),
  );
}

class _Section extends StatelessWidget {
  const _Section({required this.title, required this.children});

  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Card(
    margin: const EdgeInsets.only(bottom: 16),
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(title, style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 12),
          ...children,
        ],
      ),
    ),
  );
}

/// `TEXT`: the display can only draw `DD:DD`, so the field only takes
/// digits and a colon, five characters at most.
class _TextControl extends StatefulWidget {
  const _TextControl({required this.sender});

  final _Sender sender;

  @override
  State<_TextControl> createState() => _TextControlState();
}

class _TextControlState extends State<_TextControl> {
  final _field = TextEditingController();

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _field,
    builder: (context, _) {
      final text = _field.text;
      final error = text.isEmpty ? null : textArgError(text);
      final canSend = text.isNotEmpty && error == null;
      // Checked on use: this builder doesn't rebuild when [busy] changes.
      void send() {
        if (!widget.sender.busy) widget.sender.send(SetText(text));
      }

      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: TextField(
                  controller: _field,
                  decoration: InputDecoration(
                    labelText: 'Show text',
                    hintText: '12:34',
                    errorText: error,
                  ),
                  keyboardType: TextInputType.datetime,
                  inputFormatters: [
                    FilteringTextInputFormatter.allow(RegExp('[0-9:]')),
                    LengthLimitingTextInputFormatter(5),
                  ],
                  onSubmitted: canSend ? (_) => send() : null,
                ),
              ),
              const SizedBox(width: 12),
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: _SendButton(
                  sender: widget.sender,
                  label: 'Show',
                  onPressed: canSend ? send : null,
                ),
              ),
            ],
          ),
          _OutcomeLine(widget.sender),
        ],
      );
    },
  );
}

/// `COLOR`: a few presets plus red/green/blue sliders for anything else.
class _ColorControl extends StatefulWidget {
  const _ColorControl({required this.sender});

  final _Sender sender;

  @override
  State<_ColorControl> createState() => _ColorControlState();
}

class _ColorControlState extends State<_ColorControl> {
  static const _presets = [
    Color(0xffffffff),
    Color(0xffff0000),
    Color(0xffff8000),
    Color(0xffffff00),
    Color(0xff00ff00),
    Color(0xff00ffff),
    Color(0xff0000ff),
    Color(0xffff00ff),
    _bootColor,
  ];

  Color _color = _bootColor;

  int _channel(double c) => (c * 255).round();

  @override
  Widget build(BuildContext context) {
    final red = _channel(_color.r);
    final green = _channel(_color.g);
    final blue = _channel(_color.b);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final preset in _presets)
              _Swatch(
                color: preset,
                selected: preset == _color,
                onTap: () => setState(() => _color = preset),
              ),
          ],
        ),
        const SizedBox(height: 8),
        _ChannelSlider(
          label: 'R',
          value: red,
          onChanged: (v) => setState(() => _color = _color.withRed(v)),
        ),
        _ChannelSlider(
          label: 'G',
          value: green,
          onChanged: (v) => setState(() => _color = _color.withGreen(v)),
        ),
        _ChannelSlider(
          label: 'B',
          value: blue,
          onChanged: (v) => setState(() => _color = _color.withBlue(v)),
        ),
        Row(
          children: [
            Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                color: _color,
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: Theme.of(context).dividerColor),
              ),
            ),
            const Spacer(),
            _SendButton(
              sender: widget.sender,
              label: 'Set color',
              onPressed: () => widget.sender.send(SetColor(red, green, blue)),
            ),
          ],
        ),
        _OutcomeLine(widget.sender),
      ],
    );
  }
}

class _Swatch extends StatelessWidget {
  const _Swatch({
    required this.color,
    required this.selected,
    required this.onTap,
  });

  final Color color;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onTap,
    customBorder: const CircleBorder(),
    child: Container(
      width: 32,
      height: 32,
      decoration: BoxDecoration(
        color: color,
        shape: BoxShape.circle,
        border: Border.all(
          color: selected
              ? Theme.of(context).colorScheme.primary
              : Theme.of(context).dividerColor,
          width: selected ? 3 : 1,
        ),
      ),
    ),
  );
}

class _ChannelSlider extends StatelessWidget {
  const _ChannelSlider({
    required this.label,
    required this.value,
    required this.onChanged,
  });

  final String label;
  final int value;
  final ValueChanged<int> onChanged;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      SizedBox(width: 16, child: Text(label)),
      Expanded(
        child: Slider(
          value: value.toDouble(),
          max: 255,
          divisions: 255,
          onChanged: (v) => onChanged(v.round()),
        ),
      ),
      SizedBox(width: 32, child: Text('$value', textAlign: TextAlign.end)),
    ],
  );
}

/// `BRIGHTNESS`: sent once the slider is let go, not on every step.
class _BrightnessControl extends StatefulWidget {
  const _BrightnessControl({required this.sender});

  final _Sender sender;

  @override
  State<_BrightnessControl> createState() => _BrightnessControlState();
}

class _BrightnessControlState extends State<_BrightnessControl> {
  double _level = _bootBrightness;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Row(
        children: [
          const Icon(Icons.brightness_low),
          Expanded(
            child: Slider(
              value: _level,
              max: 255,
              divisions: 255,
              label: '${_level.round()}',
              onChanged: (v) => setState(() => _level = v),
              onChangeEnd: (v) => widget.sender.send(SetBrightness(v.round())),
            ),
          ),
          const Icon(Icons.brightness_high),
        ],
      ),
      _OutcomeLine(widget.sender),
    ],
  );
}

/// `WIFI`: joins the display to a network. The password stays masked, is
/// cleared once sent, and never leaves this form except inside the
/// Command written to the display.
class _WifiControl extends StatefulWidget {
  const _WifiControl({required this.sender});

  final _Sender sender;

  @override
  State<_WifiControl> createState() => _WifiControlState();
}

class _WifiControlState extends State<_WifiControl> {
  final _form = GlobalKey<FormState>();
  final _ssid = TextEditingController();
  final _password = TextEditingController();

  @override
  void dispose() {
    _ssid.dispose();
    _password.dispose();
    super.dispose();
  }

  void _join() {
    if (!_form.currentState!.validate()) return;
    final command = JoinWifi(_ssid.text, _password.text);
    _password.clear();
    widget.sender.send(command);
  }

  @override
  Widget build(BuildContext context) => Form(
    key: _form,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextFormField(
          controller: _ssid,
          decoration: const InputDecoration(labelText: 'Network name (SSID)'),
          autocorrect: false,
          enableSuggestions: false,
          validator: (v) => ssidError(v ?? ''),
        ),
        TextFormField(
          controller: _password,
          decoration: const InputDecoration(labelText: 'Password'),
          obscureText: true,
          autocorrect: false,
          enableSuggestions: false,
          validator: (v) => passwordError(v ?? ''),
        ),
        const SizedBox(height: 16),
        Align(
          alignment: Alignment.centerLeft,
          child: _SendButton(
            sender: widget.sender,
            label: 'Join network',
            onPressed: _join,
          ),
        ),
        _OutcomeLine(widget.sender),
      ],
    ),
  );
}
