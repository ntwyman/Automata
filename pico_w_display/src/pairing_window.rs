//! The GP22-armed Bondable Window (ADR-0002, `CONTEXT.md`'s Bondable Window
//! entry): a 45s period after a button press during which `bt.rs` allows a
//! new Pairing to persist a Bond. Outside the window, connections still
//! encrypt but [`bt::run`](crate::bt::run) leaves them non-bondable, so
//! pairing produces only a transient link.
//!
//! GP22 needs no software debounce: ticket #4's hardware validation (see
//! `bin/button_check.rs`) found it a clean, active-low GPIO with no bounce
//! artifacts on real presses, so a plain falling-edge wait is all that's
//! needed here.

use core::cell::Cell;

use defmt::info;
use embassy_rp::gpio::Input;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant};

/// How long a GP22 press keeps the window armed.
pub const WINDOW: Duration = Duration::from_secs(45);

/// Shared armed/expiry state: [`run`] (driven by the button) writes it,
/// `bt.rs`'s advertise loop reads it synchronously via [`is_armed`] right
/// before accepting a connection.
pub struct BondableWindow {
    armed_until: Mutex<CriticalSectionRawMutex, Cell<Option<Instant>>>,
}

impl BondableWindow {
    pub const fn new() -> Self {
        Self {
            armed_until: Mutex::new(Cell::new(None)),
        }
    }

    /// True iff a press within the last [`WINDOW`] hasn't yet expired.
    pub fn is_armed(&self) -> bool {
        self.armed_until
            .lock(|cell| matches!(cell.get(), Some(until) if Instant::now() < until))
    }

    fn arm(&self) {
        let until = Instant::now() + WINDOW;
        self.armed_until.lock(|cell| cell.set(Some(until)));
    }
}

/// Watches GP22 forever, arming `window` on every press (falling edge —
/// active-low). Runs alongside `bt::run`/`display_fut` in `main.rs`'s
/// top-level join; never returns.
pub async fn run(mut button: Input<'_>, window: &BondableWindow) {
    loop {
        button.wait_for_falling_edge().await;
        info!("GP22 pressed: bondable window armed for {}s", WINDOW.as_secs());
        window.arm();
    }
}
