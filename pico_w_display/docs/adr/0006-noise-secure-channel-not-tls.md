# Wi-Fi Sessions over a Noise `NNpsk0` Secure Channel, not TLS

The Wi-Fi Transport authenticates and encrypts every Session with `Noise_NNpsk0_25519_ChaChaPoly_SHA256`. The pre-shared key is the Link Key, which the phone gets over the Bonded BLE link when it claims the device. Each Command and reply line then travels as one length-prefixed Noise transport message.

We chose this over TLS because no workable option exists on either end:
- `embedded-tls`, the mainstream no_std TLS 1.3 crate, is client-only.
- The only no_std TLS server we found, GatoPSKTLS, is a v0.1 fork that supports PSK only, with no forward secrecy.
- A certificate-based TLS server has no mature no_std implementation, and would also leave the device needing a certificate the phone can trust.
- Dart's `SecureSocket` doesn't expose TLS-PSK cipher suites.

Noise `NNpsk0` is a published pattern with test vectors. It's built from well-used primitives available on both sides (RustCrypto on the device, `cryptography` in Dart). Its ephemeral X25519 exchange gives forward secrecy, which PSK-only TLS wouldn't. The trust problem (getting the key to the phone) rides on the BLE encryption the device already has.

## Consequences

- The Secure Channel is our own protocol framing around a standard handshake. Anyone adding a client (a CLI tool, Home Assistant, MQTT bridging) has to implement Noise `NNpsk0`; ordinary TLS tooling won't work.
- Exactly one Link Key exists, so exactly one client identity exists. Supporting more phones would mean per-client keys, which is a new decision.
