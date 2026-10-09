---
status: accepted
supersedes: ADR-0002
---

# First-come claiming, with non-Bonded links refused

An Unclaimed device (no Bond) persists the first Pairing as its Bond and becomes Claimed. No button is involved. It stays Claimed until a Factory Reset, which is either a 5s hold of Button A (GP12) or `RESET` from the Claimed phone. While Claimed, the device disconnects any BLE link whose encryption isn't against the stored Bond before a Session starts.

This replaces ADR-0002's GP22-gated Bondable Window. GP22 is the BOOTSEL button and shouldn't double as a user control, and a button press made first-time setup awkward for no real gain. The device is meant for the owner's own home, so trusting the first phone during the short window between unboxing (or a reset) and claiming is an acceptable exposure. A Factory Reset needs either physical access or the Claimed phone, so a stranger can't silently re-open that window.

ADR-0002 accepted that an unarmed Pairing could still finish as a transient, non-persisted encrypted link able to run Commands. That gap is now closed, because BLE hands out the Link Key (`KEY`). A transient link that could fetch it would give a stranger persistent Wi-Fi access, which is exactly what bonding control exists to prevent.

## Considered Options

- **Keep a button gate, moved to Button A.** This is still safe, but setup still needs a press, and a Factory Reset already proves physical access when re-claiming.
- **A PIN or passkey shown on the grid.** This is stronger against a stranger in radio range during the Unclaimed window. It was rejected for now: it adds a second UI flow, and the threat model is the owner's own desk.
