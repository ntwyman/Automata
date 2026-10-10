//! Flash-backed persistence for the single BLE [`BondInformation`] record
//! (ADR-0005 / `CONTEXT.md`'s Bond entry): `main.rs` calls
//! [`BondStore::load`] once at boot so `bt.rs` can re-register a
//! previously-bonded phone, and `bt.rs` calls [`BondStore::save`] when the
//! Pairing that Claims an Unclaimed device produces its Bond — there is only
//! ever one. [`BondStore::erase`] is the other side: a Factory Reset
//! (`factory_reset.rs`).
//!
//! Backed by `sequential-storage`'s wear-levelled map over the `BOND_STORAGE`
//! region `memory.x` reserves (4 x 4 KiB sectors). There's only ever one
//! record, so every store uses the same fixed `()` key; `sequential-storage`
//! rotates the underlying sector on each write so no single sector takes all
//! the wear.

use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage, PostcardValue};
use serde::{Deserialize, Serialize};
use trouble_host::prelude::BondInformation;

use crate::flash::{self, FlashHandle, SharedFlash};

/// Headroom for one `postcard`-serialized [`StoredBond`] plus
/// `sequential-storage`'s own item framing (the key itself, `()`, serializes
/// to zero bytes) — deliberately generous for a value this small.
const BUF_LEN: usize = 64;

/// Wraps the foreign [`BondInformation`] so it can implement the foreign
/// [`PostcardValue`] marker trait (the orphan rule blocks implementing one
/// foreign trait for another foreign type directly). Serializing straight
/// from trouble-host's own `#[derive(Serialize, Deserialize)]` fields (via
/// `postcard`) means a field trouble-host adds later round-trips for free,
/// unlike a hand-packed byte layout that would need to be kept in sync by
/// hand.
#[derive(Serialize, Deserialize)]
struct StoredBond(BondInformation);

impl PostcardValue<'_> for StoredBond {}

type BondMap = MapStorage<(), FlashHandle, Cache<Uncached, Uncached, Uncached, ()>>;

/// Owns the flash region reserved for the Bond record.
pub struct BondStore {
    map: BondMap,
    buf: [u8; BUF_LEN],
}

impl BondStore {
    /// Takes a handle onto the flash `settings.rs` also stores in.
    pub fn new(flash: &'static SharedFlash) -> Self {
        let flash = flash::handle(flash);

        // `memory.x`-provided symbols giving the `BOND_STORAGE` region's
        // offsets (not absolute addresses) — the form `embassy_rp::flash::
        // Flash` expects. Their *addresses*, not any value stored at them,
        // are what we want; same idiom `embassy-boot` uses for its own
        // linker-defined partitions.
        unsafe extern "C" {
            static __bond_storage_start: u32;
            static __bond_storage_end: u32;
        }
        let range = unsafe {
            let start = &__bond_storage_start as *const u32 as u32;
            let end = &__bond_storage_end as *const u32 as u32;
            start..end
        };

        let map = MapStorage::new(flash, MapConfig::new(range), Cache::new_uncached());
        Self {
            map,
            buf: [0; BUF_LEN],
        }
    }

    /// Reads the persisted Bond, if any. Call once at boot, before
    /// advertising starts, and pass the result to
    /// [`trouble_host::Stack::add_bond_information`] so an already-bonded
    /// phone reconnects with zero button presses, surviving the power cycle
    /// that just happened.
    pub async fn load(&mut self) -> Option<BondInformation> {
        match self.map.fetch_item::<StoredBond>(&mut self.buf, &()).await {
            Ok(Some(StoredBond(bond))) => Some(bond),
            Ok(None) => None,
            Err(_) => {
                defmt::warn!("bond flash read failed");
                None
            }
        }
    }

    /// Persists `bond`, overwriting any previously-stored Bond — there is
    /// only ever one. Call from `bt.rs`'s `PairingComplete { bond: Some(_) }`
    /// handler, which only fires with `Some` while the device is Unclaimed
    /// (both sides bondable).
    ///
    /// A brief (tens-of-ms, once per successful pairing) system-wide hiccup
    /// is expected here: `embassy_rp::flash`'s erase/write both run in a
    /// critical section — disconnecting flash from XIP execution requires
    /// it, not a choice made here — which will glitch the WS2812 output and
    /// any concurrent cyw43 SPI traffic for that span. Unlike the `load()`
    /// DMA read `main.rs` deliberately keeps off the boot-time critical
    /// path, this can't be scheduled around: it only happens on a user-
    /// triggered pairing, and there's no way to persist a Bond without it.
    pub async fn save(&mut self, bond: &BondInformation) {
        let record = StoredBond(bond.clone());
        match self.map.store_item(&mut self.buf, &(), &record).await {
            Ok(()) => defmt::info!("bond persisted to flash"),
            Err(_) => defmt::warn!("bond flash write failed"),
        }
    }

    /// Erases the whole Bond region, for a Factory Reset. Like
    /// [`BondStore::save`], briefly glitches the LED output and cyw43 SPI
    /// traffic.
    pub async fn erase(&mut self) -> Result<(), &'static str> {
        match self.map.erase_all().await {
            Ok(()) => {
                defmt::info!("bond flash erased");
                Ok(())
            }
            Err(_) => {
                defmt::warn!("bond flash erase failed");
                Err("flash erase failed")
            }
        }
    }
}
