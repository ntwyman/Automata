//! Factory Reset (`CONTEXT.md`): erase every persisted item and reboot
//! Unclaimed. Two ways in — holding Button A for
//! [`factory_reset_hold::HOLD_MS`](pico_w_display::factory_reset_hold::HOLD_MS), watched by
//! [`watch_button`], or `RESET` from any Session, via [`FactoryResetRequest`] —
//! and both end in [`run`].
//!
//! Button A (GP12) is active low, like GP22 was. It needs no separate
//! debounce: a bounce at either edge reads as a press too short to light a
//! column, which [`factory_reset_hold::hold_at`] already treats as a no-op.

use defmt::{info, warn};
use embassy_futures::select::{Either, select};
use embassy_rp::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use pico_w_display::protocol::FactoryReset;
use pico_w_display::factory_reset_hold::{self, Hold};

use crate::bond_store::BondStore;
use crate::settings::SettingsStore;

/// How often [`watch_button`] re-checks a held button, well under one
/// column's worth of [`factory_reset_hold::HOLD_MS`].
const TICK: Duration = Duration::from_millis(50);

/// How long [`run`] waits before wiping, so `RESET`'s `OK` (notified over
/// BLE, or written to USB) actually leaves before the reboot drops the link.
const REPLY_GRACE: Duration = Duration::from_millis(500);

/// A Factory Reset has been asked for, by `RESET` or a full hold.
pub type FactoryResetSignal = Signal<CriticalSectionRawMutex, ()>;

/// The latest [`Hold`] for the display loop to draw: [`Hold::Idle`] means
/// go back to the Wall Clock.
pub type HoldSignal = Signal<CriticalSectionRawMutex, Hold>;

/// Implements [`FactoryReset`] for `RESET`, one per Session.
#[derive(Clone, Copy)]
pub struct FactoryResetRequest<'a>(pub &'a FactoryResetSignal);

impl FactoryReset for FactoryResetRequest<'_> {
    fn request(&mut self) {
        self.0.signal(());
    }
}

/// Watches Button A forever, publishing each change in its [`Hold`] to
/// `hold` and asking `reset` for a Factory Reset once a hold completes. A
/// short press does nothing (reserved for cycling Faces).
pub async fn watch_button(mut button: Input<'_>, hold: &HoldSignal, reset: &FactoryResetSignal) -> ! {
    loop {
        button.wait_for_low().await;
        let pressed = Instant::now().as_millis();
        let mut released = None;
        let mut shown = Hold::Idle;
        loop {
            let now = Instant::now().as_millis();
            let state = factory_reset_hold::hold_at(pressed, released, now, crate::GRID_WIDTH);
            if state != shown {
                hold.signal(state);
                shown = state;
            }
            if state == Hold::Complete {
                info!("button A held: factory reset");
                reset.signal(());
                // `run` reboots the device; there's nothing left to watch.
                core::future::pending::<()>().await;
            }
            if released.is_some() {
                break;
            }
            if let Either::First(()) = select(button.wait_for_high(), Timer::after(TICK)).await {
                released = Some(Instant::now().as_millis());
            }
        }
    }
}

/// Waits for a Factory Reset request, then erases the Bond and every
/// setting and reboots. A failed erase is logged and the reboot goes ahead:
/// the device comes back with whatever survived, and the owner can retry.
pub async fn run(
    reset: &FactoryResetSignal,
    bonds: &Mutex<CriticalSectionRawMutex, BondStore>,
    settings: &Mutex<CriticalSectionRawMutex, SettingsStore>,
) -> ! {
    reset.wait().await;
    info!("factory reset requested");
    Timer::after(REPLY_GRACE).await;
    if bonds.lock().await.erase().await.is_err() {
        warn!("factory reset: bond not erased");
    }
    if settings.lock().await.erase().await.is_err() {
        warn!("factory reset: settings not erased");
    }
    info!("factory reset: rebooting");
    cortex_m::peripheral::SCB::sys_reset()
}
