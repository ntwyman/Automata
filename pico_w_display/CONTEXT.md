# pico_w_display

Firmware for the Plasma 2350 W LED matrix: renders a clock or client-set text/color, controlled over one or more transports.

## Language

**Transport**:
A channel a client can open a Session over — USB CDC-ACM serial or the BLE GATT link. `run_session` is written once against `embedded_io_async::Read`/`Write` and is identical regardless of which Transport carries it.
_Avoid_: Connection type, interface

**Session**:
One client's connected lifetime on a Transport: read a command line, dispatch it, wait for the display to ack, write back a reply. Ends when the Transport errors or closes; the caller then waits for a new one.
_Avoid_: Connection, request

**Command**:
A parsed instruction a Session dispatches to the display loop: `TEXT`, `CLOCK`, `COLOR`, `BRIGHTNESS`, `WIFI`, `UNPAIR`. Transport-agnostic — the display loop only ever sees a `Command`, never which Transport it arrived over.

**Bond**:
The persisted cryptographic pairing record (identity + LTK) that lets a previously-paired phone re-establish an encrypted BLE link with no user action. Exactly one Bond is stored at a time; a fresh pairing overwrites it.
_Avoid_: Pairing (see below), credential

**Pairing**:
The act of a phone and the device negotiating encryption for the first time. A completed pairing only produces a persisted Bond when it happens inside an armed Bondable Window; otherwise it's a transient encrypted link that leaves no Bond behind.
_Avoid_: Bonding (use Bond for the noun, Pairing for the verb/event)

**Bondable Window**:
The timed period (default 45s) after a GP22 button press during which a new Pairing is allowed to produce a persisted Bond. Outside the window, connections are still encryptable but not bondable.
_Avoid_: Pairing mode, admission window
