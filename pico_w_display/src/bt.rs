//! BLE GATT transport: bridges a `command`/`reply` characteristic pair to
//! `protocol::run_session`, exactly as `usb.rs` bridges USB CDC-ACM.
//!
//! `run_session` (`protocol.rs`) is deliberately transport-agnostic and is
//! not touched here — this module only adapts GATT writes/notifications to
//! the `embedded_io_async::Read`/`Write` traits it already expects, via
//! [`BleReader`]/[`BleWriter`].
//!
//! An Unclaimed device (no Bond) makes every connection bondable, and the
//! first Pairing's Bond is persisted via [`BondStore`], Claiming it
//! (ADR-0005) — so that phone reconnects with zero button presses,
//! surviving a power cycle. A Claimed device makes no connection bondable,
//! so no other phone can replace the Bond; only a Factory Reset does.
//!
//! Only the Claimed phone may send Commands ([`LinkGuard`]): a link
//! encrypted with the stored Bond, or the one whose Pairing just Claimed
//! the device. Any other link is disconnected before a Command reaches its
//! Session.
use core::cell::Cell;

use defmt::{info, warn};
use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embedded_io_async::{ErrorType, Read, Write};
use heapless::Vec;
use pico_w_display::link_guard::{LinkEvent, LinkGuard, Verdict};
use pico_w_display::protocol::{self, FactoryReset, TzStore, WallClock, WifiControl};
use trouble_host::prelude::*;

use crate::bond_store::BondStore;

/// [`CommandService`]'s UUID (`e3fcb01d-9492-4fa7-97db-63f3491b3f58` — a
/// fresh random 128-bit UUID, not a standard GATT profile) as 16 bytes in
/// Bluetooth's little-endian advertising order (the reverse of the UUID
/// string's big-endian/RFC4122 byte order). Spelled out again as a literal
/// on `#[gatt_service]` below since that macro needs a literal string, not a
/// `const` path.
const SERVICE_UUID_LE: [u8; 16] = [
    0x58, 0x3f, 0x1b, 0x49, 0xf3, 0x63, 0xdb, 0x97, 0xa7, 0x4f, 0x92, 0x94, 0x1d, 0xb0, 0xfc, 0xe3,
];

const DEVICE_NAME: &str = "Plasma 2350 W";

/// Backing size for the `command`/`reply` characteristics — matches
/// `protocol.rs`'s own line-length bounds so there's one source of truth.
const CMD_LEN: usize = protocol::MAX_LINE_LEN;
const REPLY_LEN: usize = protocol::MAX_REPLY_LEN;
/// One extra byte over [`CMD_LEN`] for the synthetic trailing `\n`
/// [`gatt_events_task`] appends to each GATT write before handing it to
/// [`protocol::read_line`]'s line-terminator scan (see `BleReader`).
const RX_BUF_LEN: usize = CMD_LEN + 1;

/// Max simultaneous connections — matches the single-bond-slot, single-app
/// design (`CONTEXT.md`'s Bond entry): one device, one phone.
const CONNECTIONS_MAX: usize = 1;
/// Signal + ATT — no other L2CAP channels are used.
const L2CAP_CHANNELS_MAX: usize = 2;

/// The concrete BLE controller for this board: the CYW43439's Bluetooth
/// radio (via `wifi::init`) wrapped for `trouble-host`'s HCI layer. `10` is
/// the outstanding-command slot count, matching the `rp-pico-2-w` reference.
pub type BtController = ExternalController<cyw43::bluetooth::BtDriver<'static>, 10>;

/// A single GATT write's raw bytes plus a synthetic trailing `\n`, queued
/// from [`gatt_events_task`] for [`BleReader`] to drain.
type BleRxChannel = Channel<CriticalSectionRawMutex, Vec<u8, RX_BUF_LEN>, 2>;

/// Signaled by [`gatt_events_task`] when the connection drops, so
/// [`BleReader`] can unblock a pending `read()` with an error instead of
/// [`select`] externally cancelling `run_session` — which could otherwise
/// drop it while its Command is in flight on the
/// [`protocol::DisplayMailbox`] and orphan the eventual ack for an unrelated
/// later session to wrongly consume.
type Disconnected = Signal<CriticalSectionRawMutex, ()>;

#[gatt_server]
struct Server {
    command_service: CommandService,
}

/// The custom command/reply service. Both characteristics require an
/// encrypted link — plaintext writes/subscriptions are rejected by
/// `trouble-host` before they ever reach [`gatt_events_task`].
#[gatt_service(uuid = "e3fcb01d-9492-4fa7-97db-63f3491b3f58")]
struct CommandService {
    /// One GATT write = one full command line's ASCII bytes (well under the
    /// negotiated MTU; see [`CMD_LEN`]).
    #[characteristic(
        uuid = "e7880fa0-ab1f-4d65-bf1b-88ef1176393b",
        write,
        write_without_response,
        permissions(encrypted)
    )]
    command: Vec<u8, CMD_LEN>,
    /// One notification = one full reply line.
    #[characteristic(
        uuid = "cce376e9-e3c2-41e1-8367-6c2bcf55784f",
        notify,
        permissions(encrypted)
    )]
    reply: Vec<u8, REPLY_LEN>,
}

/// A transport error that can't carry a useful reason — the failure (a
/// too-long reply, or `notify()` failing, e.g. because the peer disconnected
/// mid-write) is already logged at the point it happens; this just satisfies
/// `embedded_io_async::ErrorType`, which requires `core::error::Error`.
#[derive(Debug)]
struct BleIoError;

impl core::fmt::Display for BleIoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "BLE notify failed")
    }
}

impl core::error::Error for BleIoError {}

impl embedded_io::Error for BleIoError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

/// Adapts [`BleRxChannel`] to `embedded_io_async::Read`, one byte at a time —
/// exactly what `protocol::read_line` asks of any transport. Also watches
/// [`Disconnected`] so a dropped connection ends `run_session` through its
/// own, natural read-error path (see [`Disconnected`]'s doc comment) rather
/// than via external cancellation.
struct BleReader<'a> {
    rx: &'a BleRxChannel,
    disconnected: &'a Disconnected,
    buf: Vec<u8, RX_BUF_LEN>,
    pos: usize,
}

impl ErrorType for BleReader<'_> {
    type Error = BleIoError;
}

impl Read for BleReader<'_> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, BleIoError> {
        if self.pos >= self.buf.len() {
            match select(self.rx.receive(), self.disconnected.wait()).await {
                Either::First(buf) => {
                    self.buf = buf;
                    self.pos = 0;
                }
                Either::Second(()) => return Err(BleIoError),
            }
        }
        out[0] = self.buf[self.pos];
        self.pos += 1;
        Ok(1)
    }
}

/// Adapts the `reply` characteristic to `embedded_io_async::Write`: one
/// `write()` call notifies the connection with the full buffer in a single
/// GATT notification (`protocol.rs`'s reply buffer, [`REPLY_LEN`], always
/// fits one packet).
struct BleWriter<'a> {
    conn: &'a GattConnection<'a, 'a, DefaultPacketPool>,
    reply: &'a Characteristic<Vec<u8, REPLY_LEN>>,
}

impl ErrorType for BleWriter<'_> {
    type Error = BleIoError;
}

impl Write for BleWriter<'_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, BleIoError> {
        let value = Vec::from_slice(buf).map_err(|_| BleIoError)?;
        self.reply
            .notify(self.conn, &value, true)
            .await
            .map_err(|_| BleIoError)?;
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), BleIoError> {
        Ok(())
    }
}

/// Runs the `trouble-host` BLE stack forever: advertises continuously,
/// accepts one connection at a time, and bridges it to
/// [`protocol::run_session`] until disconnect, then advertises again.
///
/// Structurally parallel to `main.rs`'s USB session loop — `wifi` is a
/// [`crate::wifi::SharedWifi`] handle so it can run concurrently with the USB
/// session without both needing a simultaneous `&mut Wifi`.
///
/// `initial_bond` (loaded from flash by `main.rs` before this task starts)
/// is registered with the stack immediately, so a previously-bonded phone
/// can reconnect on the very first advertisement — no button press, even
/// right after a power cycle. Without one the device is Unclaimed, and
/// `bond_store` is where the Pairing that Claims it gets persisted.
/// `clock` answers `TIME`, `tz` stores `TZ`, and `reset` starts `RESET`'s
/// Factory Reset.
// One parameter per thing a Session or the Bond lifecycle needs, each
// already its own small handle; bundling them would only move the count.
#[allow(clippy::too_many_arguments)]
pub async fn run<J: WifiControl, C: WallClock, Z: TzStore, F: FactoryReset>(
    controller: BtController,
    display: &protocol::DisplayMailbox,
    mut wifi: J,
    clock: &C,
    mut tz: Z,
    initial_bond: Option<BondInformation>,
    bond_store: &Mutex<CriticalSectionRawMutex, BondStore>,
    mut reset: F,
) {
    // Fixed rather than derived from the chip's real BT MAC (no accessor for
    // it is wired up here) — fine for a single-device-per-app product; matches
    // the `embassy-rs/trouble` reference examples' own approach.
    let address = Address::random([0xff, 0x8f, 0x2c, 0x05, 0xe4, 0xff]);
    info!("ble address = {}", address);

    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(address)
        .build();
    let runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: DEVICE_NAME,
        appearance: &appearance::UNKNOWN,
    }))
    .unwrap();

    // The Claimed phone's identity, once there's a Bond: `None` means
    // Unclaimed. Only ever goes from `None` to `Some` — a Factory Reset
    // reboots rather than clearing it.
    let known_identity: Cell<Option<Identity>> = Cell::new(initial_bond.as_ref().map(|b| b.identity));
    if let Some(bond) = initial_bond {
        info!("restoring persisted bond: {}", bond);
        if stack.add_bond_information(bond).is_err() {
            warn!("failed to restore persisted bond");
        }
    }

    join(ble_host_task(runner), async {
        loop {
            match advertise(&mut peripheral, &server).await {
                Ok(conn) => {
                    info!("ble connected");
                    // Must happen before any pairing traffic arrives (see
                    // `set_bondable`'s own doc comment) — right after accept
                    // is the earliest point available. A Claimed device's
                    // link can still pair, just not bond, and
                    // `gatt_events_task` then disconnects it.
                    let _ = conn.raw().set_bondable(known_identity.get().is_none());

                    let rx: BleRxChannel = Channel::new();
                    let disconnected: Disconnected = Signal::new();
                    let command = &server.command_service.command;
                    let reply = &server.command_service.reply;

                    let events_fut =
                        gatt_events_task(&conn, command, &rx, &disconnected, bond_store, &known_identity);
                    let reader = BleReader {
                        rx: &rx,
                        disconnected: &disconnected,
                        buf: Vec::new(),
                        pos: 0,
                    };
                    let writer = BleWriter { conn: &conn, reply };
                    let session_fut = protocol::run_session(
                        reader, writer, display, &mut wifi, &mut reset, clock, &mut tz,
                    );

                    // `join`, not `select`: `session_fut` must run to its own
                    // completion (see `Disconnected`'s doc comment) rather
                    // than being cancelled the instant `events_fut` notices
                    // the disconnect.
                    join(events_fut, session_fut).await;
                    info!("ble disconnected");
                }
                Err(_) => {
                    warn!("ble advertise error");
                    Timer::after(Duration::from_millis(500)).await;
                }
            }
        }
    })
    .await;
}

/// Runs the `trouble-host` HCI/link-layer event loop; must run for as long
/// as the stack is in use. See the doc comment on the equivalent task in
/// `embassy-rs/trouble`'s example apps for why this can't just be spawned
/// generically as a `#[embassy_executor::task]`.
async fn ble_host_task(mut runner: Runner<'_, BtController, DefaultPacketPool>) {
    loop {
        if let Err(e) = runner.run().await {
            warn!("ble host runner error");
            let _ = e;
        }
    }
}

/// Advertises continuously (satisfies "always connectable" — reconnecting an
/// already-bonded phone needs no button press) and waits for one connection.
///
/// Generic over `C: Controller` (rather than fixed to [`BtController`]) only
/// to keep `C::Error`'s projection simple — `run` always calls this with the
/// one concrete controller type.
async fn advertise<'values, 'server, C: Controller>(
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv_data = [0; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[SERVICE_UUID_LE]),
        ],
        &mut adv_data[..],
    )?;
    // Kept out of `adv_data`: a 128-bit service UUID plus flags already
    // fills most of the 31-byte advertising-packet budget, and the scan
    // response has its own separate 31 bytes.
    let mut scan_data = [0; 31];
    let scan_len = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(DEVICE_NAME.as_bytes())],
        &mut scan_data[..],
    )?;

    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_data[..adv_len],
                scan_data: &scan_data[..scan_len],
            },
        )
        .await?;
    info!("ble advertising");
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    Ok(conn)
}

/// Streams GATT events for one connection: forwards the Claimed phone's
/// `command` writes into `rx` (with a synthetic trailing `\n`, matching how
/// `protocol::read_line` expects one command per terminated line) and
/// answers every GATT request, until the connection drops. Any other link
/// is disconnected, per [`LinkGuard`], and its writes refused.
async fn gatt_events_task(
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    command: &Characteristic<Vec<u8, CMD_LEN>>,
    rx: &BleRxChannel,
    disconnected: &Disconnected,
    bond_store: &Mutex<CriticalSectionRawMutex, BondStore>,
    known_identity: &Cell<Option<Identity>>,
) {
    let mut guard = LinkGuard::new();
    let refuse = |verdict: Verdict| {
        if verdict == Verdict::Disconnect {
            warn!("ble link is not the claimed phone's: disconnecting");
            conn.raw().disconnect();
        }
    };
    loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                info!("ble disconnected: {:?}", reason);
                disconnected.signal(());
                return;
            }
            GattConnectionEvent::Encrypted { security_level, bond } => {
                info!("ble encrypted: {:?}", security_level);
                // `bond` is only `Some` when the link resumed a stored Bond,
                // and the stack only ever holds the Claimed phone's. A fresh
                // Pairing reports `None` here and is judged on its
                // `PairingComplete` instead.
                if let Some(bond) = bond
                    && known_identity.get().is_some_and(|id| id.match_identity(&bond.identity))
                {
                    refuse(guard.on(LinkEvent::ResumedBond));
                }
            }
            GattConnectionEvent::PairingComplete { security_level, bond } => {
                info!("ble pairing complete: {:?}", security_level);
                // `bond` is only `Some` when both sides were bondable, which
                // `run` allows only while Unclaimed: this Pairing Claims the
                // device. Any other Pairing completes with `bond: None`,
                // persists nothing, and is disconnected.
                let claimed = match bond {
                    Some(bond) if known_identity.get().is_none() => {
                        known_identity.set(Some(bond.identity));
                        bond_store.lock().await.save(&bond).await;
                        info!("claimed");
                        true
                    }
                    _ => false,
                };
                refuse(guard.on(LinkEvent::Paired { claimed }));
            }
            GattConnectionEvent::PairingFailed(err) => {
                warn!("ble pairing failed: {:?}", err);
            }
            GattConnectionEvent::Gatt { event } => {
                let verdict = match &event {
                    GattEvent::Write(write) if write.handle() == command.handle => {
                        let verdict = guard.on(LinkEvent::Command);
                        if verdict == Verdict::Forward {
                            let line = write.with_data(|_offset, data| {
                                let mut buf: Vec<u8, RX_BUF_LEN> = Vec::new();
                                let _ = buf.extend_from_slice(data);
                                let _ = buf.push(b'\n');
                                buf
                            });
                            rx.send(line).await;
                        }
                        verdict
                    }
                    _ => Verdict::Continue,
                };
                let reply = match verdict {
                    Verdict::Disconnect => event.reject(AttErrorCode::INSUFFICIENT_AUTHENTICATION),
                    Verdict::Continue | Verdict::Forward => event.accept(),
                };
                match reply {
                    Ok(reply) => reply.send().await,
                    Err(_) => warn!("ble gatt reply error"),
                }
                refuse(verdict);
            }
            _ => {}
        }
    }
}
