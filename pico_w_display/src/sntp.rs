//! SNTP (RFC 4330) client packet handling, with no I/O: [`build_request`]
//! makes the 48-byte request, [`parse_reply`] validates the server's answer
//! and turns it into Unix seconds, and [`retry_delay`] is the backoff
//! schedule after a failed Sync. The UDP/DNS side lives in the on-device
//! `ntp.rs`.
//!
//! Accuracy is whole seconds: the reply's transmit timestamp is taken as-is,
//! with no round-trip compensation — plenty for an `HH:MM` display.

/// Length of an SNTP packet with no extension fields or authenticator.
pub const PACKET_LEN: usize = 48;

/// UDP port SNTP servers listen on.
pub const PORT: u16 = 123;

/// Seconds from the NTP epoch (1900-01-01) to the Unix epoch (1970-01-01).
const NTP_TO_UNIX: u64 = 2_208_988_800;

/// Seconds in one NTP era (the 32-bit seconds field's wraparound).
const ERA_SECS: u64 = 1 << 32;

const MODE_CLIENT: u8 = 3;
const MODE_SERVER: u8 = 4;
const VERSION: u8 = 4;
const LEAP_UNSYNCHRONIZED: u8 = 3;

const ORIGIN_OFFSET: usize = 24;
const TRANSMIT_OFFSET: usize = 40;

/// Why a reply didn't produce a Sync. Each is safe to log; none is fatal —
/// the caller just backs off and tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Reject {
    /// Fewer than [`PACKET_LEN`] bytes.
    TooShort,
    /// Mode field isn't "server".
    NotServer,
    /// Stratum 0: a Kiss-o'-Death packet (e.g. `RATE`, `DENY`).
    KissOfDeath,
    /// Stratum above 15, i.e. not a valid synchronized server.
    BadStratum,
    /// Leap indicator 3: the server's own clock isn't synchronized.
    ServerUnsynchronized,
    /// Origin timestamp isn't the transmit timestamp we sent, so this isn't
    /// the reply to our request.
    OriginMismatch,
    /// Transmit timestamp is zero.
    ZeroTransmit,
    /// The time it gives is before the firmware was built.
    BeforeBuild,
}

/// The 48-byte SNTPv4 client request. `transmit` goes in the transmit
/// timestamp field, which the server echoes back as the reply's origin
/// timestamp; it's an opaque nonce here (we have no clock to put there), so
/// any value unique per request works, as long as it's non-zero.
pub fn build_request(transmit: u64) -> [u8; PACKET_LEN] {
    let mut packet = [0u8; PACKET_LEN];
    packet[0] = (VERSION << 3) | MODE_CLIENT;
    packet[TRANSMIT_OFFSET..TRANSMIT_OFFSET + 8].copy_from_slice(&transmit.to_be_bytes());
    packet
}

/// Validates `reply` against the request [`build_request`] made with
/// `transmit`, returning the server's transmit time as Unix seconds.
/// Anything before `min_unix` — the start of the firmware's build year — is
/// rejected as a broken or hostile server rather than displayed.
///
/// The 32-bit NTP seconds field wraps on 2036-02-07; following RFC 4330 §3,
/// values with the top bit clear are taken to be in era 1 (2036–2104) and
/// values with it set in era 0 (1968–2036).
pub fn parse_reply(reply: &[u8], transmit: u64, min_unix: u64) -> Result<u64, Reject> {
    if reply.len() < PACKET_LEN {
        return Err(Reject::TooShort);
    }
    let leap = reply[0] >> 6;
    let mode = reply[0] & 0b111;
    let stratum = reply[1];
    let field = |offset: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&reply[offset..offset + 8]);
        u64::from_be_bytes(bytes)
    };

    if mode != MODE_SERVER {
        return Err(Reject::NotServer);
    }
    if stratum == 0 {
        return Err(Reject::KissOfDeath);
    }
    if stratum > 15 {
        return Err(Reject::BadStratum);
    }
    if leap == LEAP_UNSYNCHRONIZED {
        return Err(Reject::ServerUnsynchronized);
    }
    if field(ORIGIN_OFFSET) != transmit {
        return Err(Reject::OriginMismatch);
    }
    let server_transmit = field(TRANSMIT_OFFSET);
    if server_transmit == 0 {
        return Err(Reject::ZeroTransmit);
    }

    let ntp_secs = server_transmit >> 32;
    let era_secs = if ntp_secs & 0x8000_0000 == 0 {
        ERA_SECS
    } else {
        0
    };
    // `None` is an era-0 time before 1970, so before any build year too.
    match (ntp_secs + era_secs).checked_sub(NTP_TO_UNIX) {
        Some(unix) if unix >= min_unix => Ok(unix),
        _ => Err(Reject::BeforeBuild),
    }
}

/// How long to wait before the next Sync attempt after `failures`
/// consecutive failed ones (`failures >= 1`): 15s, doubling each time,
/// capped at 15 minutes.
pub fn retry_delay(failures: u32) -> u64 {
    const FIRST: u64 = 15;
    const CAP: u64 = 15 * 60;
    let doublings = failures.saturating_sub(1).min(16);
    (FIRST << doublings).min(CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-01-01T00:00:00Z — a stand-in for "the build year started".
    const MIN_UNIX: u64 = 1_767_225_600;
    /// 2026-10-03T12:34:56Z.
    const NOW_UNIX: u64 = 1_791_030_896;
    const NONCE: u64 = 0x0123_4567_89ab_cdef;

    /// A valid server reply to a request sent with [`NONCE`], carrying
    /// `ntp_secs` (with a non-zero fraction) as its transmit time.
    fn reply(ntp_secs: u32) -> [u8; PACKET_LEN] {
        let mut p = [0u8; PACKET_LEN];
        p[0] = (VERSION << 3) | MODE_SERVER;
        p[1] = 2;
        p[ORIGIN_OFFSET..ORIGIN_OFFSET + 8].copy_from_slice(&NONCE.to_be_bytes());
        let transmit = ((ntp_secs as u64) << 32) | 0x8000_0000;
        p[TRANSMIT_OFFSET..TRANSMIT_OFFSET + 8].copy_from_slice(&transmit.to_be_bytes());
        p
    }

    fn ntp(unix: u64) -> u32 {
        ((unix + NTP_TO_UNIX) % ERA_SECS) as u32
    }

    #[test]
    fn request_is_v4_client_carrying_nonce() {
        let p = build_request(NONCE);
        assert_eq!(p[0], 0b00_100_011);
        assert_eq!(&p[TRANSMIT_OFFSET..], &NONCE.to_be_bytes());
        assert!(p[1..TRANSMIT_OFFSET].iter().all(|&b| b == 0));
    }

    #[test]
    fn valid_reply_gives_unix_seconds() {
        assert_eq!(parse_reply(&reply(ntp(NOW_UNIX)), NONCE, MIN_UNIX), Ok(NOW_UNIX));
    }

    #[test]
    fn extra_trailing_bytes_are_ignored() {
        let mut long = [0u8; PACKET_LEN + 20];
        long[..PACKET_LEN].copy_from_slice(&reply(ntp(NOW_UNIX)));
        assert_eq!(parse_reply(&long, NONCE, MIN_UNIX), Ok(NOW_UNIX));
    }

    #[test]
    fn rejects_short_packet() {
        let r = reply(ntp(NOW_UNIX));
        assert_eq!(
            parse_reply(&r[..PACKET_LEN - 1], NONCE, MIN_UNIX),
            Err(Reject::TooShort)
        );
    }

    #[test]
    fn rejects_non_server_mode() {
        for mode in [0, 1, 2, 3, 5, 6, 7] {
            let mut r = reply(ntp(NOW_UNIX));
            r[0] = (VERSION << 3) | mode;
            assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Err(Reject::NotServer), "mode {mode}");
        }
    }

    #[test]
    fn rejects_kiss_of_death() {
        let mut r = reply(ntp(NOW_UNIX));
        r[1] = 0;
        assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Err(Reject::KissOfDeath));
    }

    #[test]
    fn rejects_stratum_above_15() {
        let mut r = reply(ntp(NOW_UNIX));
        r[1] = 16;
        assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Err(Reject::BadStratum));
        r[1] = 15;
        assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Ok(NOW_UNIX));
    }

    #[test]
    fn rejects_leap_indicator_3() {
        let mut r = reply(ntp(NOW_UNIX));
        r[0] |= 0b11 << 6;
        assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Err(Reject::ServerUnsynchronized));
    }

    #[test]
    fn accepts_leap_indicators_0_to_2() {
        for leap in 0..3u8 {
            let mut r = reply(ntp(NOW_UNIX));
            r[0] |= leap << 6;
            assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Ok(NOW_UNIX), "leap {leap}");
        }
    }

    #[test]
    fn rejects_origin_mismatch() {
        assert_eq!(
            parse_reply(&reply(ntp(NOW_UNIX)), NONCE + 1, MIN_UNIX),
            Err(Reject::OriginMismatch)
        );
    }

    #[test]
    fn rejects_zero_transmit() {
        let mut r = reply(ntp(NOW_UNIX));
        r[TRANSMIT_OFFSET..].fill(0);
        assert_eq!(parse_reply(&r, NONCE, MIN_UNIX), Err(Reject::ZeroTransmit));
    }

    #[test]
    fn rejects_time_before_build_year() {
        assert_eq!(
            parse_reply(&reply(ntp(MIN_UNIX - 1)), NONCE, MIN_UNIX),
            Err(Reject::BeforeBuild)
        );
        assert_eq!(parse_reply(&reply(ntp(MIN_UNIX)), NONCE, MIN_UNIX), Ok(MIN_UNIX));
    }

    #[test]
    fn rejects_era_0_time_before_unix_epoch() {
        // 1968-01-20: the earliest instant RFC 4330's era rule maps to.
        assert_eq!(
            parse_reply(&reply(0x8000_0000), NONCE, MIN_UNIX),
            Err(Reject::BeforeBuild)
        );
    }

    #[test]
    fn era_1_rollover() {
        // 2036-02-07T06:28:16Z is NTP second 2^32: the 32-bit field reads 0.
        const ROLLOVER_UNIX: u64 = 2_085_978_496;
        assert_eq!(
            parse_reply(&reply(u32::MAX), NONCE, MIN_UNIX),
            Ok(ROLLOVER_UNIX - 1)
        );
        assert_eq!(parse_reply(&reply(0), NONCE, MIN_UNIX), Ok(ROLLOVER_UNIX));
        assert_eq!(parse_reply(&reply(1), NONCE, MIN_UNIX), Ok(ROLLOVER_UNIX + 1));
    }

    #[test]
    fn retry_backs_off_to_15_minutes() {
        let delays: std::vec::Vec<u64> = (1..=9).map(retry_delay).collect();
        assert_eq!(delays, [15, 30, 60, 120, 240, 480, 900, 900, 900]);
        assert_eq!(retry_delay(u32::MAX), 900);
    }
}
