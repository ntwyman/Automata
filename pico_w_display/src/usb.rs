//! USB CDC-ACM (virtual serial port) setup.
//!
//! Everything here borrows from buffers owned by the caller (normally locals in
//! `main`), so nothing needs `'static` storage or a spawned task — `main`'s async
//! fn already lives for the whole program. That keeps this phase simple; a later
//! Wi-Fi transport (which needs a spawned `embassy-net` stack task) is where
//! `'static` storage will actually become necessary.

use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_usb::class::cdc_acm::{BufferedReceiver, CdcAcmClass, Sender, State};
use embassy_usb::{Builder, Config, UsbDevice};

/// Maximum size of a single USB packet for the CDC-ACM data endpoints.
pub const MAX_PACKET_SIZE: u16 = 64;

/// Scratch buffers the USB stack needs. Owned by the caller so the borrows can
/// tie back to a local variable's lifetime instead of requiring `'static`.
pub struct UsbBuffers {
    config_descriptor: [u8; 256],
    bos_descriptor: [u8; 256],
    control_buf: [u8; 64],
    read_buf: [u8; MAX_PACKET_SIZE as usize],
}

impl UsbBuffers {
    pub const fn new() -> Self {
        Self {
            config_descriptor: [0; 256],
            bos_descriptor: [0; 256],
            control_buf: [0; 64],
            read_buf: [0; MAX_PACKET_SIZE as usize],
        }
    }
}

/// The pieces returned by [`build`]: the USB device itself, and a
/// byte-stream `Sender`/`BufferedReceiver` pair for the serial protocol.
pub type UsbParts<'d> = (
    UsbDevice<'d, Driver<'d, USB>>,
    Sender<'d, Driver<'d, USB>>,
    BufferedReceiver<'d, Driver<'d, USB>>,
);

/// Builds the USB device and its CDC-ACM serial class.
///
/// The returned `UsbDevice`'s `run()` future must be polled continuously to
/// service the bus.
pub fn build<'d>(
    driver: Driver<'d, USB>,
    buffers: &'d mut UsbBuffers,
    state: &'d mut State<'d>,
) -> UsbParts<'d> {
    let mut config = Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("Automata");
    config.product = Some("pico_w_display");
    config.serial_number = Some("1");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut builder = Builder::new(
        driver,
        config,
        &mut buffers.config_descriptor,
        &mut buffers.bos_descriptor,
        &mut [], // no MS OS descriptors needed
        &mut buffers.control_buf,
    );

    let class = CdcAcmClass::new(&mut builder, state, MAX_PACKET_SIZE);
    let usb = builder.build();

    let (sender, receiver) = class.split();
    let receiver = receiver.into_buffered(&mut buffers.read_buf);

    (usb, sender, receiver)
}
