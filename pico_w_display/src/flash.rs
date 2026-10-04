//! The one `embassy_rp::flash::Flash`, shared by every flash-backed store
//! (`bond_store.rs`, `settings.rs`). Each store owns a `sequential-storage`
//! map, and each map wants a flash it can own outright, so each gets a
//! [`Partition`] handle onto the same [`SharedFlash`] instead.

use embassy_embedded_hal::flash::partition::Partition;
use embassy_rp::Peri;
use embassy_rp::flash::{Async, Flash};
use embassy_rp::peripherals::{DMA_CH3, FLASH};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use static_cell::StaticCell;

use crate::Irqs;

/// Matches the physical flash size `memory.x` assumes (a Pico 2 W has at
/// least this much internal flash) — independent of how much of it the
/// linker hands to code vs. the storage regions; see `embassy-rp`'s own
/// flash examples, which size their `Flash` the same way.
const FLASH_TOTAL_SIZE: usize = 2 * 1024 * 1024;

pub type SharedFlash = Mutex<CriticalSectionRawMutex, Flash<'static, FLASH, Async, FLASH_TOTAL_SIZE>>;

/// A handle onto all of [`SharedFlash`], so each store keeps addressing its
/// own linker-defined region by its offset into flash, exactly as it would
/// with a `Flash` of its own.
pub type FlashHandle = Partition<'static, CriticalSectionRawMutex, Flash<'static, FLASH, Async, FLASH_TOTAL_SIZE>>;

/// Takes the `FLASH` peripheral and a spare DMA channel (`DMA_CH3` —
/// `DMA_CH0`/`1`/`2` are already spoken for by the WS2812 output and the
/// Wi-Fi/BT SPI link; see `wifi.rs`'s module docs). Call once.
pub fn init(flash: Peri<'static, FLASH>, dma: Peri<'static, DMA_CH3>) -> &'static SharedFlash {
    static SHARED: StaticCell<SharedFlash> = StaticCell::new();
    SHARED.init(Mutex::new(Flash::new(flash, dma, Irqs)))
}

/// A new handle onto `flash`, for one store's map.
pub fn handle(flash: &'static SharedFlash) -> FlashHandle {
    Partition::new(flash, 0, FLASH_TOTAL_SIZE as u32)
}
