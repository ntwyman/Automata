//! Flash-backed device settings — the TZ Rule, the Saved Network, the
//! Link Key, and the display's brightness and colour — and the TZ Rule in
//! force.
//!
//! Backed by `sequential-storage`'s wear-levelled map over the
//! `SETTINGS_STORAGE` region `memory.x` reserves (4 x 4 KiB sectors), one
//! item per [`Key`] — unlike `bond_store.rs`'s single-record map, so several
//! settings can share it. `main.rs` calls [`SettingsStore::load_tz`] once at
//! boot and publishes the result to [`TZ_RULE`]; `TZ` (via [`TzSetting`])
//! persists a new rule then publishes it. The Saved Network is loaded at
//! boot too, but `wifi.rs` owns it from then on; so is the Link Key, which
//! `link_key.rs` owns. Brightness and colour are loaded at boot before the
//! first frame, and the display loop persists each `BRIGHTNESS` and `COLOR`.
//!
//! The Saved Network's password is stored in plaintext; see
//! `docs/adr/0004-plaintext-saved-network.md`. So is the Link Key; see
//! `docs/adr/0007-plaintext-link-key.md`.

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::watch::Watch;
use heapless::String;
use pico_w_display::protocol::{LINK_KEY_LEN, MAX_PASSWORD_LEN, MAX_SSID_LEN, TzStore};
use pico_w_display::tz::{MAX_TZ_LEN, TzRule};
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage, PostcardValue};
use serde::{Deserialize, Serialize};
use smart_leds::RGB8;

use crate::flash::{self, FlashHandle, SharedFlash};

/// Each setting's key in the map. Never renumber one: it's what's in flash.
#[derive(Clone, Copy)]
#[repr(u8)]
enum Key {
    /// The TZ Rule's text, as sent.
    Tz = 1,
    /// The Saved Network, as a [`StoredNetwork`].
    Network = 2,
    /// The Link Key's raw bytes.
    #[allow(clippy::enum_variant_names)] // The glossary's term.
    LinkKey = 3,
    /// The brightness as one byte.
    Brightness = 4,
    /// The foreground colour as three bytes: red, green, blue.
    Color = 5,
}

/// The largest [`StoredNetwork`]: each `str` is a one-byte `postcard`
/// length (both fit under 128) then its bytes.
const MAX_NETWORK_LEN: usize = 1 + MAX_SSID_LEN + 1 + MAX_PASSWORD_LEN;

/// Headroom for the largest item (a full Saved Network, or a [`MAX_TZ_LEN`]
/// TZ Rule) plus `sequential-storage`'s item framing and key.
const BUF_LEN: usize = 128;
const _: () = assert!(MAX_TZ_LEN + 16 <= BUF_LEN);
const _: () = assert!(MAX_NETWORK_LEN + 16 <= BUF_LEN);
const _: () = assert!(LINK_KEY_LEN + 16 <= BUF_LEN);

/// The network a successful `WIFI` joined, kept so the device can Rejoin it
/// at boot or after a drop. Never log `password`.
#[derive(Clone)]
pub struct SavedNetwork {
    pub ssid: String<MAX_SSID_LEN>,
    pub password: String<MAX_PASSWORD_LEN>,
}

impl SavedNetwork {
    /// `None` if either part is over its 802.11 limit.
    pub fn new(ssid: &str, password: &str) -> Option<Self> {
        let mut network = SavedNetwork {
            ssid: String::new(),
            password: String::new(),
        };
        network.ssid.push_str(ssid).ok()?;
        network.password.push_str(password).ok()?;
        Some(network)
    }
}

/// [`SavedNetwork`]'s flash form: borrowed, so it can be serialized from
/// and deserialized into `SettingsStore`'s buffer with no copies.
#[derive(Serialize, Deserialize)]
struct StoredNetwork<'a> {
    ssid: &'a str,
    password: &'a str,
}

impl<'a> PostcardValue<'a> for StoredNetwork<'a> {}

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

    /// Whether flash holds exactly `rule`'s text. Unlike [`Self::load_tz`],
    /// quiet: an unreadable or invalid record just doesn't match.
    pub async fn holds_tz(&mut self, rule: &TzRule) -> bool {
        self.holds(Key::Tz, rule.as_str().as_bytes()).await
    }

    /// Whether flash holds exactly `bytes` under `key`. Quiet: an unreadable
    /// record just doesn't match.
    async fn holds(&mut self, key: Key, bytes: &[u8]) -> bool {
        matches!(
            self.map.fetch_item::<&[u8]>(&mut self.buf, &(key as u8)).await,
            Ok(Some(stored)) if stored == bytes
        )
    }

    /// Persists `rule`, replacing any previous one. Like
    /// `bond_store::BondStore::save`, the flash write briefly glitches the
    /// LED output and cyw43 SPI traffic — fine for a `TZ` that changes the
    /// rule, which is rare ([`TzSetting`] skips unchanged ones).
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

    /// The persisted Saved Network, or `None` if there isn't one — or if
    /// what's stored can't be read, which leaves the device to wait for a
    /// `WIFI` rather than failing boot.
    pub async fn load_network(&mut self) -> Option<SavedNetwork> {
        let stored = match self
            .map
            .fetch_item::<StoredNetwork>(&mut self.buf, &(Key::Network as u8))
            .await
        {
            Ok(stored) => stored?,
            Err(_) => {
                warn!("settings flash read failed");
                return None;
            }
        };
        let network = SavedNetwork::new(stored.ssid, stored.password);
        if network.is_none() {
            warn!("stored network is invalid; ignoring it");
        }
        network
    }

    /// Persists `network` as the Saved Network, replacing any previous one.
    /// Briefly glitches the LED output and cyw43 SPI traffic, like
    /// [`SettingsStore::save_tz`].
    pub async fn save_network(&mut self, network: &SavedNetwork) -> Result<(), &'static str> {
        let stored = StoredNetwork {
            ssid: &network.ssid,
            password: &network.password,
        };
        match self.map.store_item(&mut self.buf, &(Key::Network as u8), &stored).await {
            Ok(()) => {
                info!("saved network persisted: {}", network.ssid.as_str());
                Ok(())
            }
            Err(_) => {
                warn!("settings flash write failed");
                Err("flash write failed")
            }
        }
    }

    /// The persisted Link Key, or `None` if there isn't one — or if what's
    /// stored can't be read or is the wrong length. Never log it.
    pub async fn load_link_key(&mut self) -> Option<[u8; LINK_KEY_LEN]> {
        let bytes = match self.map.fetch_item::<&[u8]>(&mut self.buf, &(Key::LinkKey as u8)).await {
            Ok(bytes) => bytes?,
            Err(_) => {
                warn!("settings flash read failed");
                return None;
            }
        };
        let key = bytes.try_into().ok();
        if key.is_none() {
            warn!("stored link key is invalid; ignoring it");
        }
        key
    }

    /// Persists `key` as the Link Key, replacing any previous one. Briefly
    /// glitches the LED output and cyw43 SPI traffic, like
    /// [`SettingsStore::save_tz`].
    pub async fn save_link_key(&mut self, key: &[u8; LINK_KEY_LEN]) -> Result<(), &'static str> {
        let bytes: &[u8] = key;
        match self.map.store_item(&mut self.buf, &(Key::LinkKey as u8), &bytes).await {
            Ok(()) => {
                info!("link key persisted");
                Ok(())
            }
            Err(_) => {
                warn!("settings flash write failed");
                Err("flash write failed")
            }
        }
    }

    /// The persisted brightness, or `None` if none was ever set — or if
    /// what's stored can't be read, which leaves the boot default.
    pub async fn load_brightness(&mut self) -> Option<u8> {
        self.load_bytes::<1>(Key::Brightness, "brightness").await.map(|[b]| b)
    }

    /// Persists `brightness`, skipping the flash write when it's unchanged:
    /// the app sends `BRIGHTNESS` every time its slider is released.
    pub async fn save_brightness(&mut self, brightness: u8) -> Result<(), &'static str> {
        if self.save_if_changed(Key::Brightness, &[brightness]).await? {
            info!("brightness persisted: {}", brightness);
        }
        Ok(())
    }

    /// The persisted foreground colour, or `None` if none was ever set — or
    /// if what's stored can't be read, which leaves the boot default.
    pub async fn load_color(&mut self) -> Option<RGB8> {
        self.load_bytes::<3>(Key::Color, "color")
            .await
            .map(|[r, g, b]| RGB8::new(r, g, b))
    }

    /// Persists `color`, skipping the flash write when it's unchanged.
    pub async fn save_color(&mut self, color: RGB8) -> Result<(), &'static str> {
        if self.save_if_changed(Key::Color, &[color.r, color.g, color.b]).await? {
            info!("color persisted: {:02x}{:02x}{:02x}", color.r, color.g, color.b);
        }
        Ok(())
    }

    /// The `N` bytes stored under `key`, or `None` if there are none, they
    /// can't be read, or they're the wrong length (logged as `what`).
    async fn load_bytes<const N: usize>(&mut self, key: Key, what: &str) -> Option<[u8; N]> {
        let bytes = match self.map.fetch_item::<&[u8]>(&mut self.buf, &(key as u8)).await {
            Ok(bytes) => bytes?,
            Err(_) => {
                warn!("settings flash read failed");
                return None;
            }
        };
        let value = bytes.try_into().ok();
        if value.is_none() {
            warn!("stored {=str} is invalid; ignoring it", what);
        }
        value
    }

    /// Stores `bytes` under `key` unless flash already holds them (reads
    /// don't wear it). `Ok(true)` if it wrote. A write briefly glitches the
    /// LED output and cyw43 SPI traffic, like [`SettingsStore::save_tz`].
    async fn save_if_changed(&mut self, key: Key, bytes: &[u8]) -> Result<bool, &'static str> {
        if self.holds(key, bytes).await {
            return Ok(false);
        }
        match self.map.store_item(&mut self.buf, &(key as u8), &bytes).await {
            Ok(()) => Ok(true),
            Err(_) => {
                warn!("settings flash write failed");
                Err("flash write failed")
            }
        }
    }

    /// Erases the whole settings region — every [`Key`], present or future —
    /// for a Factory Reset. Like [`SettingsStore::save_tz`], briefly glitches
    /// the LED output and cyw43 SPI traffic.
    pub async fn erase(&mut self) -> Result<(), &'static str> {
        match self.map.erase_all().await {
            Ok(()) => {
                info!("settings flash erased");
                Ok(())
            }
            Err(_) => {
                warn!("settings flash erase failed");
                Err("flash erase failed")
            }
        }
    }

    /// Clears the Saved Network. Idempotent: clearing when nothing is saved
    /// is not an error.
    pub async fn clear_network(&mut self) -> Result<(), &'static str> {
        match self.map.remove_item(&mut self.buf, &(Key::Network as u8)).await {
            Ok(()) => {
                info!("saved network cleared");
                Ok(())
            }
            Err(_) => {
                warn!("settings flash clear failed");
                Err("flash clear failed")
            }
        }
    }
}

/// Implements [`TzStore`] for `TZ`, shared by both the USB and BLE
/// `run_session` calls in `main.rs` — the same shape as `wifi::SharedWifi`.
#[derive(Clone, Copy)]
pub struct TzSetting<'a> {
    pub store: &'a Mutex<CriticalSectionRawMutex, SettingsStore>,
}

impl TzStore for TzSetting<'_> {
    async fn set(&mut self, rule: TzRule) -> Result<(), &'static str> {
        let mut store = self.store.lock().await;
        // The client re-sends the TZ Rule every Session, so spare the flash
        // a rewrite (reads don't wear it) when it already holds this rule.
        // Checked against flash rather than `TZ_RULE`, which is also empty
        // when the boot load failed with an older rule still stored.
        if !store.holds_tz(&rule).await {
            store.save_tz(&rule).await?;
        }
        // Published under the lock, so two Sessions' `TZ`s can't leave flash
        // holding one rule and `TZ_RULE` the other.
        TZ_RULE.sender().send(rule);
        Ok(())
    }
}
