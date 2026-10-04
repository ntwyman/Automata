//! Flash-backed persistence for the single BLE [`BondInformation`] record
//! (ticket #6 / ADR-0002 / `CONTEXT.md`'s Bond entry): `bt.rs` calls
//! [`BondStore::load`] once at boot to re-register a previously-bonded
//! phone, and [`BondStore::save`] whenever a Pairing inside the
//! [`crate::pairing_window`] produces a fresh Bond, overwriting whatever was
//! there before — there is only ever one. [`BondStore::clear`] (via
//! [`Bonds`]) is the other side: the `UNPAIR` command (ticket #7).
//!
//! Backed by `sequential-storage`'s wear-levelled map over the `BOND_STORAGE`
//! region `memory.x` reserves (4 x 4 KiB sectors). There's only ever one
//! record, so every store uses the same fixed `()` key; `sequential-storage`
//! rotates the underlying sector on each write so no single sector takes all
//! the wear.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
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
    /// handler, which only fires with `Some` when the pairing happened
    /// inside an armed Bondable Window (both sides bondable).
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

    /// Clears the persisted Bond, if any. Call from `UNPAIR`'s dispatch
    /// (via [`Bonds`]); idempotent — clearing an already-empty store is not
    /// an error.
    pub async fn clear(&mut self) -> Result<(), &'static str> {
        match self.map.remove_item(&mut self.buf, &()).await {
            Ok(()) => {
                defmt::info!("bond cleared from flash");
                Ok(())
            }
            Err(_) => {
                defmt::warn!("bond flash clear failed");
                Err("flash clear failed")
            }
        }
    }
}

/// Signaled by [`Bonds::clear`] so `bt::run`'s connection loop evicts the
/// Bond it's holding in memory (`known_identity`/`stack::
/// remove_bond_information`) once `UNPAIR` has cleared it from flash.
/// Eviction can't happen at the `UNPAIR` dispatch site itself (`usb.rs`'s or
/// `bt.rs`'s own `run_session` call) because `stack` — and the in-memory
/// Bond list it owns — lives entirely inside `bt::run`'s own local scope;
/// this signal is the one thing shared between them.
pub type UnpairSignal = Signal<CriticalSectionRawMutex, ()>;

/// Implements [`pico_w_display::protocol::BondClear`] for `UNPAIR`, shared
/// by both the USB and BLE `run_session` calls in `main.rs` — mirrors
/// `wifi::SharedWifi`'s shape for the same reason: each transport's session
/// holds its own `Bonds` handle so they can run concurrently without both
/// needing a simultaneous `&mut BondStore`.
#[derive(Clone, Copy)]
pub struct Bonds<'a> {
    pub store: &'a Mutex<CriticalSectionRawMutex, BondStore>,
    pub evict: &'a UnpairSignal,
}

impl pico_w_display::protocol::BondClear for Bonds<'_> {
    async fn clear(&mut self) -> Result<(), &'static str> {
        let result = self.store.lock().await.clear().await;
        // Only on success: `bt::run`'s eviction handler assumes flash is
        // already clear by the time this fires (see its own comment), so
        // signaling on a failed clear would evict the in-memory Bond while
        // flash still holds the old record — an inconsistent state that
        // would only self-heal on reboot.
        if result.is_ok() {
            self.evict.signal(());
        }
        result
    }
}
