//! Host-testable core of the firmware: the command protocol only.
//!
//! Everything else (display driving, USB/BLE transports, Wi-Fi/BLE chip
//! bring-up) is hardware-bound and stays in the `pico_w_display` binary
//! crate; only `protocol` — already written against `embedded_io_async`
//! traits rather than any concrete transport — lives here so `cargo test`
//! can exercise it on the host.
#![cfg_attr(not(test), no_std)]

pub mod protocol;
