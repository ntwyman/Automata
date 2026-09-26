//! Throwaway hardware spike for ticket #4: confirms GP22 (the board's
//! BOOT/user button) behaves as a plain, debounceable GPIO before any
//! Bondable-Window pairing logic is built on top of that assumption.
//!
//! Flash with `cargo run --bin button_check`, then watch the RTT log while
//! pressing the button. No internal pull is applied, so the initial level
//! and any resting-state noise reflect the pin's real, undriven behavior.

#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Pull};
use embassy_time::Instant;
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("button_check: watching GP22 (no internal pull)");

    let p = embassy_rp::init(Default::default());
    let mut button = Input::new(p.PIN_22, Pull::None);

    info!("initial level: {}", button.get_level());

    loop {
        button.wait_for_any_edge().await;
        info!(
            "t={}ms level={}",
            Instant::now().as_millis(),
            button.get_level()
        );
    }
}
