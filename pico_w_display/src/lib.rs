//! Host-testable core of the firmware: the command protocol, SNTP packet
//! handling, Wall Clock arithmetic, the TZ Rule, the Rejoin backoff
//! schedule, and Button A's Factory Reset hold.
//!
//! Everything else (display driving, USB/BLE transports, Wi-Fi/BLE chip
//! bring-up, the SNTP socket I/O) is hardware-bound and stays in the
//! `pico_w_display` binary crate; only the pure parts — `protocol`, already
//! written against `embedded_io_async` traits rather than any concrete
//! transport, plus `sntp`, `wall_clock`, `tz`, `rejoin` and `factory_reset_hold` — live here so
//! `cargo test` can exercise them on the host.
#![cfg_attr(not(test), no_std)]

pub mod factory_reset_hold;
pub mod protocol;
pub mod rejoin;
pub mod sntp;
pub mod tz;
pub mod wall_clock;
