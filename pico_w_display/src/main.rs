//! Displays elapsed time since boot in mm:ss format on a 17x17 WS2812 LED grid,
//! or whatever a client connected over USB serial asks for instead. See
//! `protocol.rs` for the command set.

#![no_std]
#![no_main]
#![allow(incomplete_features)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_futures::select::{Either, select};
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::peripherals::{DMA_CH0, PIO0, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use embassy_time::{Duration, Instant, Ticker};
use embassy_usb::class::cdc_acm::State as CdcAcmState;
use smart_leds::colors;
use {defmt_rtt as _, panic_probe as _};

mod fonts;
mod grid;
mod protocol;
mod usb;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
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
async fn main(_spawner: Spawner) {
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
        loop {
            receiver.wait_connection().await;
            info!("serial client connected");
            protocol::run_session(&mut receiver, &mut sender, &commands, &acks).await;
            info!("serial client disconnected");
        }
    };

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

    join3(usb_fut, protocol_fut, display_fut).await;
}
