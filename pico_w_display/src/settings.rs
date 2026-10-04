//! Flash-backed device settings, and the TZ Rule in force.
//!
//! Backed by `sequential-storage`'s wear-levelled map over the
//! `SETTINGS_STORAGE` region `memory.x` reserves (4 x 4 KiB sectors), one
//! item per [`Key`] — unlike `bond_store.rs`'s single-record map, so later
//! settings (e.g. saved Wi-Fi credentials) can share it. `main.rs` calls
//! [`SettingsStore::load_tz`] once at boot and publishes the result to
//! [`TZ_RULE`]; `TZ` (via [`TzSetting`]) persists a new rule then publishes it.

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::watch::Watch;
use pico_w_display::protocol::TzStore;
use pico_w_display::tz::{MAX_TZ_LEN, TzRule};
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage};

use crate::flash::{self, FlashHandle, SharedFlash};

/// Each setting's key in the map. Never renumber one: it's what's in flash.
#[repr(u8)]
enum Key {
    /// The TZ Rule's text, as sent.
    Tz = 1,
}

/// Headroom for the largest item (a [`MAX_TZ_LEN`] TZ Rule today; Wi-Fi
/// credentials later) plus `sequential-storage`'s item framing and key.
const BUF_LEN: usize = 128;
const _: () = assert!(MAX_TZ_LEN + 16 <= BUF_LEN);

type SettingsMap = MapStorage<u8, FlashHandle, Cache<Uncached, Uncached, Uncached, u8>>;

/// The TZ Rule in force, once one has been loaded at boot or set by `TZ`;
/// empty means UTC (see [`current_tz`]). One receiver: the display loop.
/// `TIME` reads it directly with `try_get`.
pub static TZ_RULE: Watch<CriticalSectionRawMutex, TzRule, 1> = Watch::new();

/// The TZ Rule in force: [`TZ_RULE`]'s, or UTC if it's empty.
pub fn current_tz() -> TzRule {
    TZ_RULE.try_get().unwrap_or_else(TzRule::utc)
}

/// Owns the flash region reserved for settings.
pub struct SettingsStore {
    map: SettingsMap,
    buf: [u8; BUF_LEN],
}

impl SettingsStore {
    /// Takes a handle onto the flash `bond_store.rs` also stores in.
    pub fn new(flash: &'static SharedFlash) -> Self {
        // `memory.x`-provided symbols giving the `SETTINGS_STORAGE` region's
        // offsets into flash; see `bond_store.rs` for the idiom.
        unsafe extern "C" {
            static __settings_storage_start: u32;
            static __settings_storage_end: u32;
        }
        let range = unsafe {
            let start = &__settings_storage_start as *const u32 as u32;
            let end = &__settings_storage_end as *const u32 as u32;
            start..end
        };

        let map = MapStorage::new(flash::handle(flash), MapConfig::new(range), Cache::new_uncached());
        Self {
            map,
            buf: [0; BUF_LEN],
        }
    }

    /// The persisted TZ Rule, or `None` if none was ever set — or if what's
    /// stored can't be read or no longer parses, which falls back to UTC
    /// rather than failing boot.
    pub async fn load_tz(&mut self) -> Option<TzRule> {
        let text = match self.map.fetch_item::<&[u8]>(&mut self.buf, &(Key::Tz as u8)).await {
            Ok(text) => text?,
            Err(_) => {
                warn!("settings flash read failed");
                return None;
            }
        };
        let rule = core::str::from_utf8(text).ok().and_then(TzRule::parse);
        if rule.is_none() {
            warn!("stored tz rule is invalid; using UTC");
        }
        rule
    }

    /// Persists `rule`, replacing any previous one. Like
    /// `bond_store::BondStore::save`, the flash write briefly glitches the
    /// LED output and cyw43 SPI traffic — fine for a rare, user-sent `TZ`.
    pub async fn save_tz(&mut self, rule: &TzRule) -> Result<(), &'static str> {
        let text = rule.as_str().as_bytes();
        match self.map.store_item(&mut self.buf, &(Key::Tz as u8), &text).await {
            Ok(()) => {
                info!("tz rule persisted: {}", rule.as_str());
                Ok(())
            }
            Err(_) => {
                warn!("settings flash write failed");
                Err("flash write failed")
            }
        }
    }
}

/// Implements [`TzStore`] for `TZ`, shared by both the USB and BLE
/// `run_session` calls in `main.rs` — the same shape as `bond_store::Bonds`.
#[derive(Clone, Copy)]
pub struct TzSetting<'a> {
    pub store: &'a Mutex<CriticalSectionRawMutex, SettingsStore>,
}

impl TzStore for TzSetting<'_> {
    async fn set(&mut self, rule: TzRule) -> Result<(), &'static str> {
        let mut store = self.store.lock().await;
        store.save_tz(&rule).await?;
        // Published under the lock, so two Sessions' `TZ`s can't leave flash
        // holding one rule and `TZ_RULE` the other.
        TZ_RULE.sender().send(rule);
        Ok(())
    }
}
