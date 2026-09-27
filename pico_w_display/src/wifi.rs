//! Wi-Fi and Bluetooth bring-up for the CYW43439 chip on the Plasma 2350 W's
//! RM2 module.
//!
//! The chip is wired the same way as a standard Pico 2 W: PIN_23 (power),
//! PIN_24 (SPI data), PIN_25 (SPI chip-select) and PIN_29 (SPI clock), driven
//! over PIO1 + DMA_CH1/DMA_CH2 so PIO0 + DMA_CH0 stay free for the WS2812
//! output. `RM2_CLOCK_DIVIDER` (rather than the plain Pico W's default
//! divider) is required for the RM2 module specifically. Both radios are
//! driven by the one `cyw43::Runner` spawned here — `bt.rs` never touches
//! PIO/SPI/DMA directly, it only gets handed the `BtDriver` this module
//! returns.
//!
//! Firmware blobs are vendored under `cyw43-firmware/` at the repo root,
//! fetched from the embassy-rs project (see the LICENSE file there).

use cyw43::bluetooth::BtDriver;
use cyw43::{Control, JoinOptions, aligned_bytes};
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_net::{Config, ConfigV4, Ipv4Address, Stack, StackResources};
use embassy_rp::Peri;
use embassy_rp::clocks::RoscRng;
use embassy_rp::dma;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH1, DMA_CH2, PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
use embassy_rp::pio::Pio;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, with_timeout};
use pico_w_display::protocol::WifiJoin;
use static_cell::StaticCell;

use crate::Irqs;

type WifiSpi = PioSpi<'static, PIO1, 0>;

/// How long to wait for association plus a DHCP lease before giving up.
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, WifiSpi>, cyw43::Cyw43439>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

/// Handle for joining a Wi-Fi network, obtained once from [`init`].
pub struct Wifi {
    control: Control<'static>,
    stack: Stack<'static>,
}

impl WifiJoin for Wifi {
    type Address = Ipv4Address;

    /// Joins `ssid` using `password` (a WPA2/WPA3 passphrase) and waits for
    /// a DHCP lease. Returns the assigned address, or a reason string that's
    /// safe to send straight back to a client as `ERR <reason>`.
    async fn join(&mut self, ssid: &str, password: &[u8]) -> Result<Ipv4Address, &'static str> {
        let outcome = with_timeout(JOIN_TIMEOUT, async {
            self.control
                .join(ssid, JoinOptions::new(password))
                .await
                .map_err(|_| "wifi join failed")?;
            // DHCP only starts now, once actually associated — `init` leaves
            // it off (see its own comment) so an unjoined device doesn't spam
            // pointless DISCOVER broadcasts every 10s.
            self.stack.set_config_v4(ConfigV4::Dhcp(Default::default()));
            self.stack.wait_config_up().await;
            Ok::<(), &'static str>(())
        })
        .await;

        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return Err(reason),
            Err(_) => return Err("wifi join timed out"),
        }

        self.stack
            .config_v4()
            .map(|c| c.address.address())
            .ok_or("no ipv4 lease")
    }
}

/// Adapts a [`Wifi`] shared behind a mutex into [`WifiJoin`], so the USB and
/// BLE sessions (`main.rs`) can run concurrently without both needing a
/// simultaneous `&mut Wifi` — each holds its own `SharedWifi` and only the
/// actual `join()` call takes the lock.
pub struct SharedWifi<'a>(pub &'a Mutex<CriticalSectionRawMutex, Wifi>);

impl WifiJoin for SharedWifi<'_> {
    type Address = Ipv4Address;

    async fn join(&mut self, ssid: &str, password: &[u8]) -> Result<Ipv4Address, &'static str> {
        self.0.lock().await.join(ssid, password).await
    }
}

/// Brings up the CYW43439 chip's Wi-Fi and Bluetooth radios and spawns the
/// one driver task that drives both. Call once from `main`; the returned
/// [`Wifi`] handle joins a network on demand, and the returned [`BtDriver`]
/// is handed to `bt::run` to build the GATT peripheral on top of.
// One parameter per distinct hardware peripheral this chip's bring-up
// actually needs — grouping them into a struct would just move the same
// count to a constructor `main.rs` still has to fill in one field at a time.
#[allow(clippy::too_many_arguments)]
pub async fn init(
    spawner: Spawner,
    pio1: Peri<'static, PIO1>,
    dma_ch1: Peri<'static, DMA_CH1>,
    dma_ch2: Peri<'static, DMA_CH2>,
    pwr_pin: Peri<'static, PIN_23>,
    dio_pin: Peri<'static, PIN_24>,
    cs_pin: Peri<'static, PIN_25>,
    clk_pin: Peri<'static, PIN_29>,
) -> (Wifi, BtDriver<'static>) {
    let fw = aligned_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = aligned_bytes!("../cyw43-firmware/43439A0_clm.bin");
    let btfw = aligned_bytes!("../cyw43-firmware/43439A0_btfw.bin");
    let nvram = aligned_bytes!("../cyw43-firmware/nvram_rp2040.bin");

    let pwr = Output::new(pwr_pin, Level::Low);
    let cs = Output::new(cs_pin, Level::High);
    let mut pio = Pio::new(pio1, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        dio_pin,
        clk_pin,
        dma::Channel::new(dma_ch1, Irqs),
        dma::Channel::new(dma_ch2, Irqs),
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    // Known risk, not a design constraint: running the Wi-Fi and Bluetooth
    // radios concurrently on this chip can hit embassy-rs/embassy#7081
    // (panic/deadlock); accepted per `docs/adr/0001-git-pinned-cyw43-bluetooth-deps.md`
    // since both radios running together is the entire point of BLE control
    // while Wi-Fi stays joined.
    let (net_device, bt_device, mut control, runner) =
        cyw43::new_with_bluetooth(state, pwr, spi, fw, btfw, nvram).await;
    spawner.spawn(unwrap!(cyw43_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // No DHCP yet: starting it before the chip has associated with any AP
    // just means `embassy-net`'s client retries a DISCOVER broadcast that
    // can never be answered, every 10s, for as long as the device sits
    // unjoined — real SPI/DMA bus traffic on the same PIO1 bus the CYW43439
    // shares between Wi-Fi and Bluetooth, contending with the WS2812
    // output's own DMA on PIO0 for no benefit. `WifiJoin::join` (above)
    // turns DHCP on only once `control.join` has actually associated.
    let config = Config::default();
    let mut rng = RoscRng;
    let seed = rng.next_u64();

    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(unwrap!(net_task(runner)));

    (Wifi { control, stack }, bt_device)
}
