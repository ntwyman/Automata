//! Displays elapsed time since boot in mm:ss format on a 17x17 WS2812 LED grid.

#![no_std]
#![no_main]
#![allow(incomplete_features)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_time::{Duration, Instant, Ticker};
use smart_leds::colors;
use {defmt_rtt as _, panic_probe as _};

mod fonts;
mod grid;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("Start");
    let p = embassy_rp::init(Default::default());

    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);
    let program = PioWs2812Program::new(&mut common);
    let ws2812 = PioWs2812::new(&mut common, sm0, p.DMA_CH0, p.PIN_15, &program);

    let mut grd = grid::Grid::<17, 289>::new(ws2812, grid::GridOrigin::TopRight);

    grd.set_background(colors::BLACK);
    grd.set_foreground(colors::DARK_BLUE);

    let start = Instant::now();
    let mut last_secs = u64::MAX; // force a draw on the first tick

    let mut ticker = Ticker::every(Duration::from_millis(100));

    loop {
        let elapsed = Instant::now() - start;
        let total_secs = elapsed.as_secs();

        if total_secs != last_secs {
            last_secs = total_secs;

            let mm = (total_secs / 60) % 100;
            let ss = total_secs % 60;

            info!("elapsed {:02}:{:02}", mm, ss);

            let digits: [u8; 4] = [
                (mm / 10) as u8,
                (mm % 10) as u8,
                (ss / 10) as u8,
                (ss % 10) as u8,
            ];

            grd.clear();

            for (i, &d) in digits.iter().enumerate() {
                grd.blit_glyph(DIGIT_COLS[i], DIGIT_Y, fonts::get_digit_glyph(d));
            }
            grd.blit_glyph(COLON_X, DIGIT_Y, fonts::get_colon_glyph());

            grd.update().await;
        }

        ticker.next().await;
    }
}
