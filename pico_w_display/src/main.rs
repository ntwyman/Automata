//! Displays the NTP-synced Wall Clock (UTC, 24-hour `HH:MM`) on a 17x17
//! WS2812 LED grid, or whatever a client connected over USB serial or BLE
//! asks for instead. See `protocol.rs` for the command set and `ntp.rs` for
//! Syncing.

#![no_std]
#![no_main]
#![allow(incomplete_features)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_futures::join::join5;
use embassy_futures::select::{Either3, select3};
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::gpio::{Input, Pull};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, PIO0, PIO1, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::class::cdc_acm::State as CdcAcmState;
use pico_w_display::{protocol, wall_clock};
use smart_leds::colors;
use trouble_host::prelude::ExternalController;
use {defmt_rtt as _, panic_probe as _};

mod bond_store;
mod bt;
mod fonts;
mod grid;
mod ntp;
mod pairing_window;
mod usb;
mod wifi;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    PIO1_IRQ_0 => PioInterruptHandler<PIO1>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>, dma::InterruptHandler<DMA_CH2>, dma::InterruptHandler<DMA_CH3>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
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

type DisplayGrid<'d> = grid::Grid<'d, 17, 289>;

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

fn render(grid: &mut DisplayGrid, frame: Frame) {
    grid.clear();
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
/// `ntp::WALL_CLOCK`), and when that next changes on its own — `None` if
/// only a Command or a Sync can change it.
fn frame_at(mode: &DisplayMode, boot_utc_ms: Option<u64>, now: Instant) -> (Frame, Option<Instant>) {
    match (mode, boot_utc_ms) {
        (DisplayMode::Text(digits), _) => (Frame::digits(*digits, true), None),
        (DisplayMode::Clock, None) => (UNSYNCED, None),
        (DisplayMode::Clock, Some(boot_utc_ms)) => {
            let utc_ms = ntp::utc_ms(boot_utc_ms, now);
            let face = wall_clock::face(utc_ms);
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

    let mut grd = grid::Grid::<17, 289>::new(ws2812, grid::GridOrigin::TopRight);

    grd.set_background(colors::BLACK);
    grd.set_foreground(colors::DARK_BLUE);

    let (wifi_dev, bt_device, net_stack) = wifi::init(
        spawner, p.PIO1, p.DMA_CH1, p.DMA_CH2, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29,
    )
    .await;
    // Idles on `wait_config_up` until a `WIFI` join gets a DHCP lease.
    spawner.spawn(unwrap!(ntp::task(net_stack)));
    // Shared rather than owned outright: the USB and BLE sessions below run
    // concurrently and each needs its own `WifiJoin` handle (see
    // `wifi::SharedWifi`), so a plain `&mut Wifi` can't work for both.
    let wifi_mutex: Mutex<CriticalSectionRawMutex, wifi::Wifi> = Mutex::new(wifi_dev);

    // Deliberately sequential, *after* Wi-Fi bring-up finishes: the async
    // flash read below runs on DMA_CH3, which shares `DMA_IRQ_0` with the
    // Wi-Fi/BT DMA channels above, and `wifi::init`'s cyw43 bring-up is a
    // timing-sensitive SPI-over-PIO exchange (firmware/NVRAM checksums) that
    // doesn't tolerate an unrelated DMA channel's interrupts landing mid-
    // transfer. Running these two concurrently (an earlier version of this
    // code did, to shave boot time) reproduced SPI corruption on real
    // hardware — not worth it for a one-time, sub-millisecond flash scan.
    let mut bond_store = bond_store::BondStore::new(p.FLASH, p.DMA_CH3);
    let initial_bond = bond_store.load().await;

    // `initial_bond` (loaded above) lets a previously-bonded phone reconnect
    // on `bt::run`'s very first advertisement — even right after this
    // power-on — with no button press. `bond_store` itself is shared
    // (rather than handed to `bt::run` outright) because a fresh pairing
    // later in this same session also needs to write to it, and `UNPAIR`
    // (over either transport) needs to clear it.
    let bond_store_mutex: Mutex<CriticalSectionRawMutex, bond_store::BondStore> = Mutex::new(bond_store);
    // Lets `UNPAIR`, dispatched from either transport's `run_session`, tell
    // `bt::run`'s connection loop to evict its in-memory Bond — see
    // `bond_store::UnpairSignal`'s doc comment for why that can't happen
    // directly at the dispatch site.
    let unpair_signal: bond_store::UnpairSignal = bond_store::UnpairSignal::new();

    // GP22 (the board's BOOT/user button): pressing it arms the Bondable
    // Window `bt::run` checks before allowing a new connection to bond (see
    // `pairing_window`'s module docs and ADR-0002). No internal pull —
    // ticket #4's hardware validation found the board already has an
    // external one.
    let button = Input::new(p.PIN_22, Pull::None);
    let bondable_window = pairing_window::BondableWindow::new();
    let pairing_window_fut = pairing_window::run(button, &bondable_window);

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
        let mut wifi = wifi::SharedWifi(&wifi_mutex);
        let mut bonds = bond_store::Bonds {
            store: &bond_store_mutex,
            evict: &unpair_signal,
        };
        loop {
            receiver.wait_connection().await;
            info!("serial client connected");
            protocol::run_session(
                &mut receiver,
                &mut sender,
                &display,
                &mut wifi,
                &mut bonds,
                &ntp::Clock,
            )
            .await;
            info!("serial client disconnected");
        }
    };

    let ble_controller: bt::BtController = ExternalController::new(bt_device);
    let ble_fut = bt::run(
        ble_controller,
        &display,
        wifi::SharedWifi(&wifi_mutex),
        &ntp::Clock,
        initial_bond,
        &bondable_window,
        bond_store::Bonds {
            store: &bond_store_mutex,
            evict: &unpair_signal,
        },
    );

    let display_fut = async {
        let mut syncs = unwrap!(ntp::WALL_CLOCK.receiver());
        let mut boot_utc_ms: Option<u64> = None;
        let mut mode = DisplayMode::Clock;
        let mut shown: Option<Frame> = None;

        loop {
            let (frame, next_change) = frame_at(&mode, boot_utc_ms, Instant::now());
            if shown != Some(frame) {
                render(&mut grd, frame);
                grd.update().await;
                shown = Some(frame);
            }

            let tick = async {
                match next_change {
                    Some(at) => Timer::at(at).await,
                    None => core::future::pending().await,
                }
            };
            match select3(tick, display.receive(), syncs.changed()).await {
                Either3::First(()) => {}
                Either3::Second(command) => {
                    match command {
                        protocol::Command::Clock => mode = DisplayMode::Clock,
                        protocol::Command::Text(s) => mode = DisplayMode::Text(text_digits(&s)),
                        protocol::Command::Color(c) => grd.set_foreground(c),
                        protocol::Command::Brightness(b) => grd.set_brightness(b),
                    }
                    // Always redrawn, even if the frame is unchanged (e.g.
                    // `COLOR`), and before the ack so `OK` means it's shown.
                    let (frame, _) = frame_at(&mode, boot_utc_ms, Instant::now());
                    render(&mut grd, frame);
                    grd.update().await;
                    shown = Some(frame);

                    display.ack().await;
                }
                Either3::Third(synced) => boot_utc_ms = Some(synced),
            }
        }
    };

    join5(usb_fut, protocol_fut, ble_fut, display_fut, pairing_window_fut).await;
}
