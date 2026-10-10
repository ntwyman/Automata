//! Displays the NTP-synced Wall Clock (24-hour `HH:MM` local time, per the
//! persisted TZ Rule) on a 17x17 WS2812 LED grid, or whatever a client
//! connected over USB serial or BLE asks for instead. See `protocol.rs` for
//! the command set, `ntp.rs` for Syncing, `settings.rs` for the TZ Rule and
//! `wifi.rs` for Rejoining the Saved Network, `link_key.rs` for the Link Key,
//! and `factory_reset.rs` for the Factory Reset.

#![no_std]
#![no_main]
#![allow(incomplete_features)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_futures::join::{join, join5};
use embassy_futures::select::{Either5, select5};
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::gpio::{Input, Pull};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, PIO0, PIO1, TRNG, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::trng::{self, Trng};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::class::cdc_acm::State as CdcAcmState;
use pico_w_display::factory_reset_hold::Hold;
use pico_w_display::tz::TzRule;
use pico_w_display::{protocol, wall_clock};
use smart_leds::colors;
use trouble_host::prelude::ExternalController;
use {defmt_rtt as _, panic_probe as _};

mod bond_store;
mod bt;
mod factory_reset;
mod flash;
mod fonts;
mod grid;
mod link_key;
mod ntp;
mod settings;
mod usb;
mod wifi;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    PIO1_IRQ_0 => PioInterruptHandler<PIO1>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>, dma::InterruptHandler<DMA_CH2>, dma::InterruptHandler<DMA_CH3>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
    TRNG_IRQ => trng::InterruptHandler<TRNG>;
});

// Display layout on 17x17 grid (digits are 3 wide x 6 tall):
//   x=0  : tens of hours
//   x=4  : units of hours
//   x=8  : colon (1 wide)
//   x=10 : tens of minutes
//   x=14 : units of minutes
//   y=5  : vertically centred ((17 - 6) / 2 = 5)
const DIGIT_Y: usize = 5;
const DIGIT_COLS: [usize; 4] = [0, 4, 10, 14];
const COLON_X: usize = 8;

/// The grid's width (and height), in LEDs.
const GRID_WIDTH: usize = 17;

/// Button A's Factory Reset progress bar: three rows, vertically centred,
/// filling left to right.
const BAR_Y: usize = 7;
const BAR_HEIGHT: usize = 3;

type DisplayGrid<'d> = grid::Grid<'d, GRID_WIDTH, { GRID_WIDTH * GRID_WIDTH }>;

/// What the grid is currently showing: the Wall Clock, or a client-set
/// string held as its four digits (colon is implicit, same position always).
enum DisplayMode {
    Clock,
    Text([u8; 4]),
}

/// One frame in the fixed clock layout: four cells (`None` draws a dash)
/// and whether the colon is lit.
#[derive(Clone, Copy, PartialEq)]
struct Frame {
    cells: [Option<u8>; 4],
    colon: bool,
}

/// `--:--` with a solid colon: the Wall Clock before the first Sync.
const UNSYNCED: Frame = Frame {
    cells: [None; 4],
    colon: true,
};

impl Frame {
    fn digits(digits: [u8; 4], colon: bool) -> Self {
        Frame {
            cells: digits.map(Some),
            colon,
        }
    }
}

/// Draws `frame`, unless Button A is being held for a Factory Reset, in
/// which case the hold's progress bar replaces it.
fn render(grid: &mut DisplayGrid, frame: Frame, hold: Hold) {
    grid.clear();
    let filled = match hold {
        Hold::Idle => None,
        Hold::Filling { filled } => Some(filled),
        Hold::Complete => Some(GRID_WIDTH),
    };
    if let Some(filled) = filled {
        for x in 0..filled {
            for y in BAR_Y..BAR_Y + BAR_HEIGHT {
                grid.set(x, y, grid.foreground());
            }
        }
        return;
    }
    for (i, cell) in frame.cells.iter().enumerate() {
        match cell {
            Some(d) => {
                if let Some(glyph) = fonts::get_digit_glyph(*d) {
                    grid.blit_glyph(DIGIT_COLS[i], DIGIT_Y, glyph);
                }
            }
            None => grid.blit_glyph(DIGIT_COLS[i], DIGIT_Y, fonts::get_dash_glyph()),
        }
    }
    if frame.colon {
        grid.blit_glyph(COLON_X, DIGIT_Y, fonts::get_colon_glyph());
    }
}

/// What `mode` shows at `now` given the last Sync (`boot_utc_ms`, see
/// `ntp::WALL_CLOCK`) and the TZ Rule, and when that next changes on its own
/// — `None` if only a Command, a Sync or a `TZ` can change it.
fn frame_at(
    mode: &DisplayMode,
    boot_utc_ms: Option<u64>,
    tz: &TzRule,
    now: Instant,
) -> (Frame, Option<Instant>) {
    match (mode, boot_utc_ms) {
        (DisplayMode::Text(digits), _) => (Frame::digits(*digits, true), None),
        (DisplayMode::Clock, None) => (UNSYNCED, None),
        (DisplayMode::Clock, Some(boot_utc_ms)) => {
            let utc_ms = ntp::utc_ms(boot_utc_ms, now);
            let face = wall_clock::face(tz.local_ms(utc_ms));
            // DST transitions land on whole seconds, so on a colon toggle too.
            let next = now + Duration::from_millis(wall_clock::ms_until_change(utc_ms));
            (Frame::digits(face.digits, face.colon), Some(next))
        }
    }
}

/// Parses the validated `DD:DD` string from [`protocol::Command::Text`] into
/// its four digits. `protocol::parse_line` already checked the shape, so the
/// digit arithmetic here can't fail.
fn text_digits(s: &str) -> [u8; 4] {
    let b = s.as_bytes();
    [b[0] - b'0', b[1] - b'0', b[3] - b'0', b[4] - b'0']
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Start");

    let p = embassy_rp::init(Default::default());

    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);
    let program = PioWs2812Program::new(&mut common);
    let ws2812 = PioWs2812::new(&mut common, sm0, p.DMA_CH0, Irqs, p.PIN_15, &program);

    let mut grd = DisplayGrid::new(ws2812, grid::GridOrigin::TopRight);

    grd.set_background(colors::BLACK);
    grd.set_foreground(colors::DARK_BLUE);

    let (mut wifi_dev, bt_device, net_stack) = wifi::init(
        spawner, p.PIO1, p.DMA_CH1, p.DMA_CH2, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29,
    )
    .await;
    // Idles on `wait_config_up` until a join gets a DHCP lease.
    spawner.spawn(unwrap!(ntp::task(net_stack)));

    // Deliberately sequential, *after* Wi-Fi bring-up finishes: the async
    // flash read below runs on DMA_CH3, which shares `DMA_IRQ_0` with the
    // Wi-Fi/BT DMA channels above, and `wifi::init`'s cyw43 bring-up is a
    // timing-sensitive SPI-over-PIO exchange (firmware/NVRAM checksums) that
    // doesn't tolerate an unrelated DMA channel's interrupts landing mid-
    // transfer. Running these two concurrently (an earlier version of this
    // code did, to shave boot time) reproduced SPI corruption on real
    // hardware — not worth it for a one-time, sub-millisecond flash scan.
    let shared_flash = flash::init(p.FLASH, p.DMA_CH3);
    let mut bond_store = bond_store::BondStore::new(shared_flash);
    let initial_bond = bond_store.load().await;
    // Same reasoning as the Bond load: sequential, after Wi-Fi bring-up.
    let mut settings_store = settings::SettingsStore::new(shared_flash);
    if let Some(rule) = settings_store.load_tz().await {
        info!("restoring tz rule: {}", rule.as_str());
        settings::TZ_RULE.sender().send(rule);
    }
    if let Some(network) = settings_store.load_network().await {
        info!("restoring saved network: {}", network.ssid.as_str());
        wifi_dev.restore(network);
    }
    let initial_link_key = settings_store.load_link_key().await;
    if let Some(key) = initial_link_key {
        info!("restoring link key");
        link_key::publish(key);
    }
    let settings_mutex: Mutex<CriticalSectionRawMutex, settings::SettingsStore> =
        Mutex::new(settings_store);

    // The default `sample_count` (25) is the datasheet's fast setting, and
    // on this board it failed the TRNG's health tests hundreds of times
    // drawing one Link Key next to an active radio. 100 is the datasheet's
    // suggestion for far fewer failures; a few ms more per Claim is nothing.
    let mut trng_config = trng::Config::default();
    trng_config.sample_count = 100;
    let mut link_keys = link_key::LinkKeys {
        trng: Trng::new(p.TRNG, Irqs, trng_config),
        settings: &settings_mutex,
    };
    // Claimed but no Link Key: claimed by firmware from before Link Keys,
    // or the Claim's key write failed. Every Claimed device has one.
    if initial_bond.is_some() && initial_link_key.is_none() {
        warn!("claimed with no link key");
        link_keys.generate().await;
    }

    // Shared rather than owned outright: the USB and BLE sessions and the
    // Rejoin supervisor below run concurrently, and each needs its own
    // handle (see `wifi::SharedWifi`), so a plain `&mut Wifi` can't work.
    let wifi_mutex: Mutex<CriticalSectionRawMutex, wifi::Wifi> = Mutex::new(wifi_dev);
    let rejoin_signal = wifi::RejoinSignal::new();
    let shared_wifi = wifi::SharedWifi {
        wifi: &wifi_mutex,
        settings: &settings_mutex,
        rejoin: &rejoin_signal,
    };
    let rejoin_fut = wifi::supervise(&wifi_mutex, net_stack, &rejoin_signal);

    // `initial_bond` (loaded above) lets a previously-bonded phone reconnect
    // on `bt::run`'s very first advertisement — even right after this
    // power-on — with no button press. `bond_store` itself is shared
    // (rather than handed to `bt::run` outright) because the Pairing that
    // Claims an Unclaimed device also needs to write to it, and a Factory
    // Reset needs to erase it.
    let bond_store_mutex: Mutex<CriticalSectionRawMutex, bond_store::BondStore> = Mutex::new(bond_store);

    // Factory Reset: `RESET` from either Session, or a 5s hold of Button A
    // (GP12, active low, with a pull-up in case the board has none),
    // whose progress the display loop draws.
    let factory_reset_signal = factory_reset::FactoryResetSignal::new();
    let hold_signal = factory_reset::HoldSignal::new();
    let button_a = Input::new(p.PIN_12, Pull::Up);
    let button_fut = factory_reset::watch_button(button_a, &hold_signal, &factory_reset_signal);
    let factory_reset_fut = factory_reset::run(&factory_reset_signal, &bond_store_mutex, &settings_mutex);

    // USB CDC-ACM serial port. All these buffers are plain locals, borrowed
    // for the rest of `main` rather than declared `'static` — nothing here is
    // spawned as a separate task, so that's all the lifetime we need.
    let driver = UsbDriver::new(p.USB, Irqs);
    let mut usb_buffers = usb::UsbBuffers::new();
    let mut usb_state = CdcAcmState::new();
    let (mut usb_dev, mut sender, mut receiver) =
        usb::build(driver, &mut usb_buffers, &mut usb_state);

    let display = protocol::DisplayMailbox::new();

    let usb_fut = usb_dev.run();

    let protocol_fut = async {
        let mut wifi = shared_wifi;
        let mut reset = factory_reset::FactoryResetRequest(&factory_reset_signal);
        let mut tz = settings::TzSetting {
            store: &settings_mutex,
        };
        loop {
            receiver.wait_connection().await;
            info!("serial client connected");
            protocol::run_session(
                &mut receiver,
                &mut sender,
                protocol::Transport::Usb,
                &display,
                &mut wifi,
                &mut reset,
                &ntp::Clock,
                &mut tz,
                &link_key::CurrentLinkKey,
            )
            .await;
            info!("serial client disconnected");
        }
    };

    let ble_controller: bt::BtController = ExternalController::new(bt_device);
    let ble_fut = bt::run(
        ble_controller,
        &display,
        shared_wifi,
        &ntp::Clock,
        settings::TzSetting {
            store: &settings_mutex,
        },
        initial_bond,
        &bond_store_mutex,
        &mut link_keys,
        factory_reset::FactoryResetRequest(&factory_reset_signal),
    );

    let display_fut = async {
        let mut syncs = unwrap!(ntp::WALL_CLOCK.receiver());
        let mut tz_changes = unwrap!(settings::TZ_RULE.receiver());
        let mut boot_utc_ms: Option<u64> = None;
        let mut tz = settings::current_tz();
        let mut mode = DisplayMode::Clock;
        let mut hold = Hold::Idle;
        let mut shown: Option<(Frame, Hold)> = None;

        loop {
            let (frame, next_change) = frame_at(&mode, boot_utc_ms, &tz, Instant::now());
            if shown != Some((frame, hold)) {
                render(&mut grd, frame, hold);
                grd.update().await;
                shown = Some((frame, hold));
            }

            let tick = async {
                match next_change {
                    Some(at) => Timer::at(at).await,
                    None => core::future::pending().await,
                }
            };
            match select5(
                tick,
                display.receive(),
                syncs.changed(),
                tz_changes.changed(),
                hold_signal.wait(),
            )
            .await
            {
                Either5::First(()) => {}
                Either5::Second(command) => {
                    match command {
                        protocol::Command::Clock => mode = DisplayMode::Clock,
                        protocol::Command::Text(s) => mode = DisplayMode::Text(text_digits(&s)),
                        protocol::Command::Color(c) => grd.set_foreground(c),
                        protocol::Command::Brightness(b) => grd.set_brightness(b),
                    }
                    // Always redrawn, even if the frame is unchanged (e.g.
                    // `COLOR`), and before the ack so `OK` means it's shown.
                    let (frame, _) = frame_at(&mode, boot_utc_ms, &tz, Instant::now());
                    render(&mut grd, frame, hold);
                    grd.update().await;
                    shown = Some((frame, hold));

                    display.ack().await;
                }
                Either5::Third(synced) => boot_utc_ms = Some(synced),
                Either5::Fourth(rule) => tz = rule,
                Either5::Fifth(state) => hold = state,
            }
        }
    };

    join(
        join5(usb_fut, protocol_fut, ble_fut, display_fut, button_fut),
        join(rejoin_fut, factory_reset_fut),
    )
    .await;
}
