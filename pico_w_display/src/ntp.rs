//! Keeps the Wall Clock Synced over SNTP, and hands the result to the
//! display loop and `TIME`.
//!
//! [`task`] Syncs as soon as the Wi-Fi link has a DHCP lease, then every 6h;
//! a failed attempt backs off per [`sntp::retry_delay`]. Losing and
//! regaining the lease (e.g. a Rejoin) cuts either wait short and Syncs
//! straight away. Each Sync publishes
//! the UTC time at `Instant` zero to [`WALL_CLOCK`] — a `Watch`, not a
//! `Command`, since nothing about it needs a reply. Between Syncs, and
//! indefinitely if later ones keep failing, the Wall Clock free-runs on the
//! last one. Packet building and validation are the pure, host-tested
//! `sntp` module; this file is only the DNS/UDP plumbing around it.

use defmt::{info, warn};
use embassy_futures::select::{Either, select};
use embassy_net::dns::DnsQueryType;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Watch;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use pico_w_display::protocol::WallClock;
use pico_w_display::tz::TzRule;
use pico_w_display::{sntp, wall_clock};

const SERVER: &str = "pool.ntp.org";
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const RESYNC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Room for a reply carrying extension fields, so one isn't dropped as
/// truncated before `sntp::parse_reply` gets to ignore them.
const RX_BUF_LEN: usize = 128;

/// When the firmware was built (Unix seconds, stamped by `build.rs`). A Sync
/// reporting a time before the start of this year is rejected.
const BUILD_UNIX_SECS: u64 = match u64::from_str_radix(env!("BUILD_UNIX_SECS"), 10) {
    Ok(secs) => secs,
    Err(_) => panic!("BUILD_UNIX_SECS is not a number"),
};

/// UTC Unix milliseconds at `Instant` zero, from the last Sync; empty while
/// Unsynced. One receiver: the display loop. `TIME` reads it directly with
/// `try_get`.
pub static WALL_CLOCK: Watch<CriticalSectionRawMutex, u64, 1> = Watch::new();

/// UTC Unix milliseconds at `now`, given the last Sync's `boot_utc_ms`.
pub fn utc_ms(boot_utc_ms: u64, now: Instant) -> u64 {
    boot_utc_ms + now.as_millis()
}

/// [`WallClock`] over [`WALL_CLOCK`] and `settings::TZ_RULE`, for `TIME`.
pub struct Clock;

impl WallClock for Clock {
    fn utc_now(&self) -> Option<u64> {
        WALL_CLOCK
            .try_get()
            .map(|boot_utc_ms| utc_ms(boot_utc_ms, Instant::now()) / 1000)
    }

    fn tz_rule(&self) -> TzRule {
        crate::settings::current_tz()
    }
}

/// Why one Sync attempt failed.
#[derive(defmt::Format)]
enum SyncError {
    Dns,
    Socket,
    Timeout,
    Rejected(sntp::Reject),
}

#[embassy_executor::task]
pub async fn task(stack: Stack<'static>) -> ! {
    let min_unix = wall_clock::year_start(BUILD_UNIX_SECS);
    let sender = WALL_CLOCK.sender();
    let mut failures = 0u32;
    loop {
        stack.wait_config_up().await;
        let wait = match sync(stack, min_unix).await {
            Ok(utc_secs) => {
                // Whole seconds, so this is the start of the reply's second.
                let boot_utc_ms = (utc_secs * 1000).saturating_sub(Instant::now().as_millis());
                sender.send(boot_utc_ms);
                info!("synced: {}", wall_clock::iso8601(utc_secs).as_str());
                failures = 0;
                RESYNC_INTERVAL
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let delay = sntp::retry_delay(failures);
                warn!("sync failed: {}; retrying in {}s", e, delay);
                Duration::from_secs(delay)
            }
        };
        // A failure was likely for want of a network, and a lost lease may
        // mean free-running for a while: either way, a network coming back
        // (a Rejoin after a router reboot, say) shouldn't wait this out.
        let relinked = async {
            stack.wait_config_down().await;
            stack.wait_config_up().await;
        };
        if let Either::Second(()) = select(Timer::after(wait), relinked).await {
            info!("network back up; syncing now");
            failures = 0;
        }
    }
}

/// One SNTP exchange with a freshly-resolved [`SERVER`] address, returning
/// its UTC time as Unix seconds.
async fn sync(stack: Stack<'static>, min_unix: u64) -> Result<u64, SyncError> {
    let addrs = stack
        .dns_query(SERVER, DnsQueryType::A)
        .await
        .map_err(|_| SyncError::Dns)?;
    let server = *addrs.first().ok_or(SyncError::Dns)?;

    let mut rx_meta = [PacketMetadata::EMPTY; 1];
    let mut rx_buf = [0u8; RX_BUF_LEN];
    let mut tx_meta = [PacketMetadata::EMPTY; 1];
    let mut tx_buf = [0u8; sntp::PACKET_LEN];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(0).map_err(|_| SyncError::Socket)?;

    // Only an opaque nonce to match the reply against (see
    // `sntp::build_request`); uptime ticks are unique per attempt and never 0
    // this long after boot.
    let nonce = Instant::now().as_ticks();
    socket
        .send_to(&sntp::build_request(nonce), (server, sntp::PORT))
        .await
        .map_err(|_| SyncError::Socket)?;

    let mut reply = [0u8; RX_BUF_LEN];
    let (len, _) = with_timeout(REPLY_TIMEOUT, socket.recv_from(&mut reply))
        .await
        .map_err(|_| SyncError::Timeout)?
        .map_err(|_| SyncError::Socket)?;
    sntp::parse_reply(&reply[..len], nonce, min_unix).map_err(SyncError::Rejected)
}
