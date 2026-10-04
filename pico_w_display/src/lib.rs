//! Host-testable core of the firmware: the command protocol, SNTP packet
//! handling, Wall Clock arithmetic, and the TZ Rule.
//!
//! Everything else (display driving, USB/BLE transports, Wi-Fi/BLE chip
//! bring-up, the SNTP socket I/O) is hardware-bound and stays in the
//! `pico_w_display` binary crate; only the pure parts — `protocol`, already
//! written against `embedded_io_async` traits rather than any concrete
//! transport, plus `sntp`, `wall_clock` and `tz` — live here so `cargo test` can
//! exercise them on the host.
#![cfg_attr(not(test), no_std)]

pub mod protocol;
pub mod sntp;
pub mod tz;
pub mod wall_clock;
