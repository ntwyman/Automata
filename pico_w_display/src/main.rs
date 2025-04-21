//! This example shows powerful PIO module in the RP2040 chip to communicate with WS2812 LED modules.
//! See (https://www.sparkfun.com/categories/tags/ws2812)

#![no_std]
#![no_main]
#![allow(incomplete_features)]
#![feature(generic_const_exprs)]

use defmt::*;
use embassy_executor::Spawner;

use embassy_time::{Duration, Ticker};
use smart_leds::colors;
use {defmt_rtt as _, panic_probe as _};

mod fonts;
mod grid;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("Start");
    let p = embassy_rp::init(Default::default());

    let mut grd =
        grid::Grid::<17, 289>::new(p.PIO0, p.DMA_CH0, p.PIN_14, grid::GridOrigin::TopRight);

    // Loop forever making RGB values and pushing them out to the WS2812.
    let mut ticker = Ticker::every(Duration::from_millis(100));

    grd.set_background(colors::YELLOW);
    grd.clear();
    grd.set_foreground(colors::DARK_BLUE);
    grd.blit_glyph(0, 0, fonts::get_glyph('2'));
    grd.blit_glyph(4, 0, fonts::get_glyph('1'));
    grd.blit_glyph(8, 0, fonts::get_glyph('0'));
    grd.update().await;
    let mut bit = true;

    loop {
        // Wait for the next tick.
        bit = !bit;
        if bit {
            info!("on");
            grd.on(16, 16);
        } else {
            info!("off");
            grd.off(16, 16);
        }
        grd.update().await;
        ticker.next().await;
    }
}
