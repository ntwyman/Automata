//! Displays elapsed time since boot in mm:ss format on a 17x17 WS2812 LED grid,
//! or whatever a client connected over USB serial asks for instead. See
//! `protocol.rs` for the command set.

#![no_std]
#![no_main]
#![allow(incomplete_features)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_futures::join::join5;
use embassy_futures::select::{Either, select};
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::gpio::{Input, Pull};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, PIO0, PIO1, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker};
use embassy_usb::class::cdc_acm::State as CdcAcmState;
use pico_w_display::protocol;
use smart_leds::colors;
use trouble_host::prelude::ExternalController;
use {defmt_rtt as _, panic_probe as _};

mod bond_store;
mod bt;
mod fonts;
mod grid;
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
//   x=0  : tens of minutes
//   x=4  : units of minutes
//   x=8  : colon (1 wide)
//   x=10 : tens of seconds
//   x=14 : units of seconds
//   y=5  : vertically centred ((17 - 6) / 2 = 5)
const DIGIT_Y: usize = 5;
const DIGIT_COLS: [usize; 4] = [0, 4, 10, 14];
const COLON_X: usize = 8;

type DisplayGrid<'d> = grid::Grid<'d, 17, 289>;

/// What the grid is currently showing: the automatic clock, or a client-set
/// string held as its four digits (colon is implicit, same position always).
enum DisplayMode {
    Clock,
    Text([u8; 4]),
}

/// Draws four digits with a colon between the pair, in the fixed clock layout.
fn render_digits(grid: &mut DisplayGrid, digits: [u8; 4]) {
    grid.clear();
    for (i, &d) in digits.iter().enumerate() {
        if let Some(glyph) = fonts::get_digit_glyph(d) {
            grid.blit_glyph(DIGIT_COLS[i], DIGIT_Y, glyph);
        }
    }
    grid.blit_glyph(COLON_X, DIGIT_Y, fonts::get_colon_glyph());
}

/// Computes (total whole seconds elapsed, its four display digits) at `now`.
fn clock_digits(now: Instant, start: Instant) -> (u64, [u8; 4]) {
    let total_secs = (now - start).as_secs();
    let mm = (total_secs / 60) % 100;
    let ss = total_secs % 60;
    (
        total_secs,
        [
            (mm / 10) as u8,
            (mm % 10) as u8,
            (ss / 10) as u8,
            (ss % 10) as u8,
        ],
    )
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

    let (wifi_dev, bt_device) = wifi::init(
        spawner, p.PIO1, p.DMA_CH1, p.DMA_CH2, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29,
    )
    .await;
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

    let commands = protocol::CommandChannel::new();
    let acks = protocol::AckChannel::new();

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
                &commands,
                &acks,
                &mut wifi,
                &mut bonds,
            )
            .await;
            info!("serial client disconnected");
        }
    };

    let ble_controller: bt::BtController = ExternalController::new(bt_device);
    let ble_fut = bt::run(
        ble_controller,
        &commands,
        &acks,
        wifi::SharedWifi(&wifi_mutex),
        initial_bond,
        &bondable_window,
        bond_store::Bonds {
            store: &bond_store_mutex,
            evict: &unpair_signal,
        },
    );

    let display_fut = async {
        let start = Instant::now();
        let mut mode = DisplayMode::Clock;
        let mut last_secs = u64::MAX; // force a draw on the first tick
        let mut ticker = Ticker::every(Duration::from_millis(100));

        loop {
            match select(ticker.next(), commands.receive()).await {
                Either::First(()) => {
                    // Ticks only drive redraws while the clock is showing;
                    // manually-set text sits still until changed again.
                    if let DisplayMode::Clock = mode {
                        let (secs, digits) = clock_digits(Instant::now(), start);
                        if secs != last_secs {
                            last_secs = secs;
                            info!(
                                "elapsed {:02}:{:02}",
                                digits[0] * 10 + digits[1],
                                digits[2] * 10 + digits[3]
                            );
                            render_digits(&mut grd, digits);
                            grd.update().await;
                        }
                    }
                }
                Either::Second(command) => {
                    match command {
                        protocol::Command::Clock => {
                            mode = DisplayMode::Clock;
                            last_secs = u64::MAX; // force an immediate redraw below
                        }
                        protocol::Command::Text(s) => mode = DisplayMode::Text(text_digits(&s)),
                        protocol::Command::Color(c) => grd.set_foreground(c),
                        protocol::Command::Brightness(b) => grd.set_brightness(b),
                    }

                    let digits = match mode {
                        DisplayMode::Clock => {
                            let (secs, digits) = clock_digits(Instant::now(), start);
                            last_secs = secs;
                            digits
                        }
                        DisplayMode::Text(digits) => digits,
                    };
                    render_digits(&mut grd, digits);
                    grd.update().await;

                    acks.send(()).await;
                }
            }
        }
    };

    join5(usb_fut, protocol_fut, ble_fut, display_fut, pairing_window_fut).await;
}
