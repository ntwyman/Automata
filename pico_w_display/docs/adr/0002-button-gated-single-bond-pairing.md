---
status: superseded by ADR-0005
---

# Button-gated, single-slot BLE bonding

The device always advertises and accepts encrypted connections, but only admits a *new* persisted Bond within a 45s Bondable Window opened by a GP22 press — otherwise pairing traffic can still complete but produces a transient, non-persisted encrypted link (`trouble-host` has no pre-negotiation reject hook, only a post-hoc `PairingComplete` report). Only one Bond is ever stored; a fresh Pairing inside the window overwrites it rather than accumulating multiple trusted phones. We picked this over always-bondable (a stranger in radio range could silently pair and gain permanent control) and over a PIN/passkey scheme (`trouble-host`'s bonding example doesn't build one in, and it adds a second UI flow for no real gain given decision #6's threat model — this is a device on the owner's own desk/network, not something resisting a targeted over-the-air attacker). The accepted gap (an unarmed transient pairing can still complete) is deliberate: closing it fully would mean disconnecting on every unarmed `PairingComplete{bond:None}`, which isn't needed to stop *persistent* stranger access, the actual goal.
