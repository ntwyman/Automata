//! Line-based text protocol for controlling the display.
//!
//! This module knows nothing about USB: it only requires `embedded_io_async`
//! `Read`/`Write` implementations, so the same [`run_session`] will work unchanged
//! once the transport becomes a Wi-Fi TCP socket instead of a USB serial port.
//!
//! One command per line, terminated by `\r` and/or `\n` (either byte ends a
//! line; a blank line — including the second half of a `\r\n`/`\n\r` pair —
//! is silently ignored rather than treated as an empty command). This means
//! it works the same whether a client sends LF, CR, or CRLF line endings, and
//! also whether a human is typing into a raw terminal like `picocom`, whose
//! Enter key sends a bare `\r` with no local echo by default. Every command
//! gets exactly one reply line: `OK` on success, or `ERR <reason>` on
//! failure. See the commands handled in [`parse_line`].
//!
//! The grammar is ASCII-only: a line holding any byte outside ASCII is
//! rejected whole with `ERR unsupported char`, never parsed.

use core::fmt::Write as _;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embedded_io_async::{Read, Write};
use heapless::String;
use smart_leds::RGB8;

use crate::tz::{MAX_TZ_LEN, TzRule};

// Host tests can't link defmt's global logger (that's `defmt-rtt`, an
// on-device crate), so these become no-ops under `cfg(test)`. On-device
// behavior is unchanged: `info!`/`warn!` still resolve to `defmt::info!`/
// `defmt::warn!` there.
#[cfg(not(test))]
use defmt::{info, warn};
#[cfg(test)]
macro_rules! info {
    ($($arg:tt)*) => {};
}
#[cfg(test)]
macro_rules! warn {
    ($($arg:tt)*) => {};
}

/// What a transport needs to provide so [`run_session`] can dispatch `WIFI`
/// and `FORGET` without depending on any concrete Wi-Fi stack. Kept separate
/// from [`Command`]/[`DisplayMailbox`] because joining or forgetting a
/// network is a direct, awaited round-trip — there's no display to ack it.
// Only implemented in this workspace (by `wifi::SharedWifi` and this
// module's own test fake), so the usual reason for the `async_fn_in_trait`
// lint — an external implementor needing `Send` bounds it can't add later —
// doesn't apply.
#[allow(async_fn_in_trait)]
pub trait WifiControl {
    /// The joined network's address, formatted straight into the `OK <addr>`
    /// reply — hence the `Display` bound.
    type Address: core::fmt::Display;

    /// Joins `ssid` using `password`, returning the assigned address, or a
    /// reason string that's safe to send straight back to a client as
    /// `ERR <reason>`.
    async fn join(&mut self, ssid: &str, password: &[u8]) -> Result<Self::Address, &'static str>;

    /// Clears the Saved Network and leaves the current one, returning a
    /// reason string that's safe to send straight back to a client as
    /// `ERR <reason>` on failure. Idempotent: forgetting with nothing saved
    /// is not an error.
    async fn forget(&mut self) -> Result<(), &'static str>;
}

/// What a transport needs to provide so [`run_session`] can dispatch
/// `RESET` without depending on the flash stores or the reboot. Unlike
/// `WIFI`/`FORGET` it's not a round-trip: `RESET` always replies `OK`, and
/// only once that reply is written does the Session ask for the Factory
/// Reset, which then erases flash and reboots on its own.
pub trait FactoryReset {
    /// Starts a Factory Reset. Returns at once; the reboot follows shortly.
    fn request(&mut self);
}

/// What a transport needs to provide so [`run_session`] can answer `TIME`
/// without depending on any concrete clock. Like `WIFI`/`RESET`, `TIME` is
/// answered directly and never reaches the display loop.
pub trait WallClock {
    /// The current UTC time as Unix seconds, or `None` while Unsynced.
    fn utc_now(&self) -> Option<u64>;

    /// The TZ Rule in force.
    fn tz_rule(&self) -> TzRule;
}

/// What a transport needs to provide so [`run_session`] can dispatch `TZ`.
/// Not a [`Command`]: the TZ Rule is persisted and also read by `TIME`, so
/// it lives outside the display loop, which picks up changes itself.
#[allow(async_fn_in_trait)]
pub trait TzStore {
    /// Persists `rule` and puts it in force, or returns a reason string
    /// that's safe to send straight back to a client as `ERR <reason>`, with
    /// the previous rule still in force.
    async fn set(&mut self, rule: TzRule) -> Result<(), &'static str>;
}

/// Bytes in the Link Key (`CONTEXT.md`): 256 bits, so `KEY` replies with
/// twice this many hex digits.
pub const LINK_KEY_LEN: usize = 32;

/// What a transport needs to provide so [`run_session`] can answer `KEY`.
/// Like [`WallClock`], read directly and never sent to the display loop.
pub trait LinkKeySource {
    /// The Link Key, or `None` while the device is Unclaimed. Never log it.
    fn link_key(&self) -> Option<[u8; LINK_KEY_LEN]>;
}

/// Which Transport a Session runs over. Every Command behaves the same on
/// each, except `KEY`: the Link Key only ever leaves over BLE (ADR-0007).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Ble,
}

/// Longest SSID 802.11 allows, and so `WIFI` accepts.
pub const MAX_SSID_LEN: usize = 32;
/// Longest WPA2/WPA3 passphrase, and so `WIFI` accepts.
pub const MAX_PASSWORD_LEN: usize = 63;

/// Longest line we'll accept; comfortably longer than any real command (the
/// longest is `WIFI <32-byte ssid> <63-byte password>`). Anything longer is
/// rejected as `ERR line too long` rather than silently truncated.
///
/// `pub` (rather than internal-only) so the on-device `[[bin]]` crate's BLE
/// `command` characteristic (`bt.rs`, a separate crate from this `[lib]`)
/// can size its backing buffer off the same bound instead of duplicating it.
pub const MAX_LINE_LEN: usize = 104;

/// Longest reply line `run_session` ever writes: `TIME`'s
/// `OK <ISO-8601 UTC> <TZ Rule>\n`. `pub` for the same reason as
/// [`MAX_LINE_LEN`]: the BLE `reply` characteristic sizes its backing buffer
/// off this bound.
pub const MAX_REPLY_LEN: usize = "OK ".len() + "2026-10-03T12:34:56Z ".len() + MAX_TZ_LEN + 1;
const _: () = assert!("OK ".len() + 2 * LINK_KEY_LEN + "\n".len() <= MAX_REPLY_LEN);

/// The display loop's side of every Session: Sessions hand it [`Command`]s,
/// and it acks each once applied so the Session knows when it's safe to
/// reply `OK`. One `DisplayMailbox` is shared by every Session, whatever its
/// Transport.
///
/// Acks are bare `()`, so nothing in an ack says whose Command it was for.
/// What ties it to the right Session is `turn`: a Session holds it from
/// sending its Command until receiving that Command's ack, so at most one
/// Session ever has a Command in flight. Another Session wanting the display
/// meanwhile just waits its turn.
///
/// Dropping a Session mid-[`DisplayMailbox::apply`] would release `turn`
/// with an ack still owed, for the next Session to take as its own — so a
/// Session must run to completion rather than being cancelled (see
/// `bt.rs`'s `Disconnected`).
pub struct DisplayMailbox {
    commands: Channel<CriticalSectionRawMutex, Command, 1>,
    acks: Channel<CriticalSectionRawMutex, (), 1>,
    turn: Mutex<CriticalSectionRawMutex, ()>,
}

impl DisplayMailbox {
    pub const fn new() -> Self {
        DisplayMailbox {
            commands: Channel::new(),
            acks: Channel::new(),
            turn: Mutex::new(()),
        }
    }

    /// Session side: waits for this Session's turn, then hands `command` to
    /// the display loop and returns once the display has applied it.
    async fn apply(&self, command: Command) {
        let _turn = self.turn.lock().await;
        self.commands.send(command).await;
        self.acks.receive().await;
    }

    /// Display loop side: the next Command to apply. Follow each with
    /// [`DisplayMailbox::ack`] once it's applied.
    pub async fn receive(&self) -> Command {
        self.commands.receive().await
    }

    /// Display loop side: the last Command from [`DisplayMailbox::receive`] has
    /// been applied.
    pub async fn ack(&self) {
        self.acks.send(()).await;
    }
}

impl Default for DisplayMailbox {
    fn default() -> Self {
        Self::new()
    }
}

/// A parsed, fully-validated client command. By the time one of these exists,
/// it is safe to apply directly to the display with no further checks.
pub enum Command {
    /// Render this exact 5-character `DD:DD`-shaped string with the existing
    /// digit/colon font, replacing the clock until [`Command::Clock`] is sent.
    Text(String<5>),
    /// Resume showing the Wall Clock.
    Clock,
    /// Set the foreground color used to draw glyphs.
    Color(RGB8),
    /// Set overall LED brightness (0-255).
    Brightness(u8),
}

/// A parsed line: either a [`Command`] destined for the display loop, or a
/// `WIFI` request handled directly in [`run_session`] since it has nothing
/// to do with the grid.
enum ParsedLine {
    Display(Command),
    Wifi(String<MAX_SSID_LEN>, String<MAX_PASSWORD_LEN>),
    /// Clear the Saved Network and leave the current one, handled directly
    /// in [`run_session`] like `Wifi`.
    Forget,
    /// Factory Reset, handled directly in [`run_session`] like `Wifi`
    /// since it has nothing to do with the grid.
    Reset,
    /// Report the Wall Clock's UTC and TZ Rule, handled directly in
    /// [`run_session`].
    Time,
    /// Persist and apply a TZ Rule, handled directly in [`run_session`].
    Tz(TzRule),
    /// Report the Link Key, handled directly in [`run_session`], and only
    /// over BLE.
    Key,
}

#[derive(defmt::Format)]
enum ProtocolError {
    UnknownCommand,
    BadArgs,
    UnsupportedChar,
    BadTz,
    NotAllowed,
}

impl ProtocolError {
    fn reason(&self) -> &'static str {
        match self {
            ProtocolError::UnknownCommand => "unknown command",
            ProtocolError::BadArgs => "bad args",
            ProtocolError::UnsupportedChar => "unsupported char",
            ProtocolError::BadTz => "bad tz",
            ProtocolError::NotAllowed => "not allowed",
        }
    }
}

/// Parses one command line. Never panics on malformed input — anything that
/// doesn't match a known command shape is a plain `Err`, not a crash. This
/// matters more once the same parser is reachable over the network.
fn parse_line(line: &str) -> Result<ParsedLine, ProtocolError> {
    let line = line.trim();
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

    if cmd.eq_ignore_ascii_case("TEXT") {
        parse_text(rest).map(ParsedLine::Display)
    } else if cmd.eq_ignore_ascii_case("CLOCK") {
        Ok(ParsedLine::Display(Command::Clock))
    } else if cmd.eq_ignore_ascii_case("COLOR") {
        parse_color(rest).map(ParsedLine::Display)
    } else if cmd.eq_ignore_ascii_case("BRIGHTNESS") {
        parse_brightness(rest).map(ParsedLine::Display)
    } else if cmd.eq_ignore_ascii_case("WIFI") {
        parse_wifi(rest)
    } else if cmd.eq_ignore_ascii_case("FORGET") {
        Ok(ParsedLine::Forget)
    } else if cmd.eq_ignore_ascii_case("RESET") {
        // Bare only: wiping the device is too drastic to guess at.
        if rest.is_empty() {
            Ok(ParsedLine::Reset)
        } else {
            Err(ProtocolError::BadArgs)
        }
    } else if cmd.eq_ignore_ascii_case("TIME") {
        Ok(ParsedLine::Time)
    } else if cmd.eq_ignore_ascii_case("TZ") {
        parse_tz(rest)
    } else if cmd.eq_ignore_ascii_case("KEY") {
        if rest.is_empty() {
            Ok(ParsedLine::Key)
        } else {
            Err(ProtocolError::BadArgs)
        }
    } else {
        Err(ProtocolError::UnknownCommand)
    }
}

/// `DD:DD` — same 5-character layout the clock already draws (two digits, a
/// colon, two digits), just with client-supplied digits.
fn parse_text(s: &str) -> Result<Command, ProtocolError> {
    let bytes = s.as_bytes();
    if bytes.len() != 5 {
        return Err(ProtocolError::BadArgs);
    }
    for (i, &b) in bytes.iter().enumerate() {
        let ok = if i == 2 {
            b == b':'
        } else {
            b.is_ascii_digit()
        };
        if !ok {
            return Err(ProtocolError::UnsupportedChar);
        }
    }
    let mut text = String::new();
    text.push_str(s).map_err(|_| ProtocolError::BadArgs)?;
    Ok(Command::Text(text))
}

/// `RRGGBB` hex color.
fn parse_color(s: &str) -> Result<Command, ProtocolError> {
    if s.len() != 6 {
        return Err(ProtocolError::BadArgs);
    }
    let byte = |range| u8::from_str_radix(&s[range], 16).map_err(|_| ProtocolError::BadArgs);
    let r = byte(0..2)?;
    let g = byte(2..4)?;
    let b = byte(4..6)?;
    Ok(Command::Color(RGB8::new(r, g, b)))
}

/// `0`-`255` brightness level.
fn parse_brightness(s: &str) -> Result<Command, ProtocolError> {
    let n: u16 = s.parse().map_err(|_| ProtocolError::BadArgs)?;
    let n = u8::try_from(n).map_err(|_| ProtocolError::BadArgs)?;
    Ok(Command::Brightness(n))
}

/// `<ssid> <password>`. The SSID can't contain spaces, but the password is
/// everything after that first space so it may (WPA2/3 passphrases allow
/// them). Bounds match the real 802.11 limits: a 32-byte SSID and a
/// 63-character passphrase.
fn parse_wifi(s: &str) -> Result<ParsedLine, ProtocolError> {
    let mut parts = s.splitn(2, ' ');
    let ssid = parts.next().unwrap_or("");
    let password = parts.next().unwrap_or("").trim();
    if ssid.is_empty() || password.is_empty() {
        return Err(ProtocolError::BadArgs);
    }

    let mut ssid_buf: String<MAX_SSID_LEN> = String::new();
    ssid_buf
        .push_str(ssid)
        .map_err(|_| ProtocolError::BadArgs)?;
    let mut password_buf: String<MAX_PASSWORD_LEN> = String::new();
    password_buf
        .push_str(password)
        .map_err(|_| ProtocolError::BadArgs)?;
    Ok(ParsedLine::Wifi(ssid_buf, password_buf))
}

/// `<posix>`: a TZ Rule in the subset [`TzRule::parse`] accepts.
fn parse_tz(s: &str) -> Result<ParsedLine, ProtocolError> {
    if s.is_empty() {
        return Err(ProtocolError::BadArgs);
    }
    TzRule::parse(s).map(ParsedLine::Tz).ok_or(ProtocolError::BadTz)
}

/// Outcome of reading one line from the transport.
enum Line {
    /// A complete line, ready to parse.
    Ready,
    /// The line exceeded [`MAX_LINE_LEN`]; bytes up to the next `\n` were
    /// discarded to resynchronize, and no attempt was made to parse it.
    TooLong,
    /// The line held a byte outside ASCII, so it was dropped unparsed. The
    /// grammar is ASCII-only, and keeping such bytes out of `buf` is what
    /// makes byte-offset slicing of a [`Line::Ready`] line safe: stored as
    /// `byte as char`, each would become a 2-byte UTF-8 char.
    NonAscii,
}

/// Reads bytes until a `\r` or `\n`, appending the line's ASCII bytes into
/// `buf` (cleared by the caller first). A terminator with no bytes before it
/// is just a blank line — most commonly the second half of a `\r\n`/`\n\r`
/// pair — and is skipped rather than reported, so CR-only, LF-only and CRLF
/// senders all work. Every byte, ASCII or not, counts toward
/// [`MAX_LINE_LEN`], and a too-long line is reported as [`Line::TooLong`]
/// even if it also held non-ASCII bytes. Returns `Err(())` on any transport
/// error, or a clean close (`read` == 0 bytes) — both mean the session is
/// over.
async fn read_line<R: Read>(reader: &mut R, buf: &mut String<MAX_LINE_LEN>) -> Result<Line, ()> {
    let mut len = 0usize;
    let mut non_ascii = false;
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).await.map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        if byte[0] == b'\r' || byte[0] == b'\n' {
            if len == 0 {
                continue;
            }
            return Ok(if len > MAX_LINE_LEN {
                Line::TooLong
            } else if non_ascii {
                Line::NonAscii
            } else {
                Line::Ready
            });
        }
        len = len.saturating_add(1);
        if !byte[0].is_ascii() {
            non_ascii = true;
        } else if len <= MAX_LINE_LEN {
            // Can't fail: at most `len` bytes have been pushed.
            let _ = buf.push(byte[0] as char);
        }
    }
}

/// Runs one client session to completion: reads lines, dispatches parsed
/// commands to the display loop over `display`, waits for the display to
/// apply each, and writes back `OK`/`ERR`. Returns once the
/// transport errors or closes (e.g. USB disconnect), so the caller can wait
/// for a new connection and call this again. `transport` is which Transport
/// `reader`/`writer` are, and matters only to `KEY`.
// One parameter per thing a Command needs, each its own small handle;
// bundling them would only move the count.
#[allow(clippy::too_many_arguments)]
pub async fn run_session<
    R: Read,
    W: Write,
    J: WifiControl,
    F: FactoryReset,
    C: WallClock,
    Z: TzStore,
    K: LinkKeySource,
>(
    mut reader: R,
    mut writer: W,
    transport: Transport,
    display: &DisplayMailbox,
    wifi: &mut J,
    factory_reset: &mut F,
    clock: &C,
    tz: &mut Z,
    link_key: &K,
) {
    let mut line: String<MAX_LINE_LEN> = String::new();
    loop {
        line.clear();
        let outcome = match read_line(&mut reader, &mut line).await {
            Ok(outcome) => outcome,
            Err(_) => {
                info!("serial session ended");
                return;
            }
        };

        let mut reply: String<MAX_REPLY_LEN> = String::new();
        let mut reset = false;
        match outcome {
            Line::TooLong => {
                let _ = reply.push_str("ERR line too long\n");
            }
            // Never logged: it could be a `WIFI` line carrying a password.
            Line::NonAscii => {
                let _ = writeln!(reply, "ERR {}", ProtocolError::UnsupportedChar.reason());
            }
            Line::Ready => {
                // Logged before parsing so malformed lines are still visible
                // — except WIFI, whose password must never hit the RTT log.
                if line.as_str().len() >= 4 && line.as_str()[..4].eq_ignore_ascii_case("wifi") {
                    info!("recv: WIFI <redacted>");
                } else {
                    info!("recv: {}", line.as_str());
                }
                match parse_line(&line) {
                    Ok(ParsedLine::Display(command)) => {
                        display.apply(command).await;
                        let _ = reply.push_str("OK\n");
                    }
                    Ok(ParsedLine::Wifi(ssid, password)) => {
                        match wifi.join(&ssid, password.as_bytes()).await {
                            Ok(ip) => {
                                let _ = writeln!(reply, "OK {}", ip);
                            }
                            Err(reason) => {
                                let _ = writeln!(reply, "ERR {}", reason);
                            }
                        }
                    }
                    Ok(ParsedLine::Forget) => match wifi.forget().await {
                        Ok(()) => {
                            let _ = reply.push_str("OK\n");
                        }
                        Err(reason) => {
                            let _ = writeln!(reply, "ERR {}", reason);
                        }
                    },
                    Ok(ParsedLine::Reset) => {
                        let _ = reply.push_str("OK\n");
                        reset = true;
                    }
                    Ok(ParsedLine::Time) => match clock.utc_now() {
                        Some(utc) => {
                            let _ = writeln!(
                                reply,
                                "OK {} {}",
                                crate::wall_clock::iso8601(utc),
                                clock.tz_rule().as_str()
                            );
                        }
                        None => {
                            let _ = reply.push_str("ERR not synced\n");
                        }
                    },
                    Ok(ParsedLine::Tz(rule)) => match tz.set(rule).await {
                        Ok(()) => {
                            let _ = reply.push_str("OK\n");
                        }
                        Err(reason) => {
                            let _ = writeln!(reply, "ERR {}", reason);
                        }
                    },
                    // The reply is never logged, so the key stays off RTT.
                    Ok(ParsedLine::Key) if transport != Transport::Ble => {
                        let _ = writeln!(reply, "ERR {}", ProtocolError::NotAllowed.reason());
                    }
                    Ok(ParsedLine::Key) => match link_key.link_key() {
                        Some(key) => {
                            let _ = reply.push_str("OK ");
                            for byte in key {
                                let _ = write!(reply, "{:02x}", byte);
                            }
                            let _ = reply.push('\n');
                        }
                        None => {
                            let _ = reply.push_str("ERR not claimed\n");
                        }
                    },
                    Err(e) => {
                        let _ = writeln!(reply, "ERR {}", e.reason());
                    }
                }
            }
        }

        let written = writer.write_all(reply.as_bytes()).await;
        // After the `OK` is written, so the client sees it before the
        // reboot drops the link — but even if it couldn't be: the owner
        // asked, and the reply is only a courtesy.
        if reset {
            factory_reset.request();
        }
        if written.is_err() {
            warn!("serial write failed, ending session");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embassy_futures::select::select;

    use super::*;

    /// Feeds fixed bytes one at a time, then reports a clean close (`Ok(0)`)
    /// once exhausted — exactly how `read_line` sees a transport disconnect,
    /// which is what ends [`run_session`]'s loop in every test here.
    struct FakeReader {
        bytes: std::vec::Vec<u8>,
        pos: usize,
    }

    impl embedded_io_async::ErrorType for FakeReader {
        type Error = Infallible;
    }

    impl Read for FakeReader {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
            if self.pos >= self.bytes.len() {
                return Ok(0);
            }
            buf[0] = self.bytes[self.pos];
            self.pos += 1;
            Ok(1)
        }
    }

    /// Collects everything written to it into a caller-owned buffer, so the
    /// buffer is still readable once `run_session` (which takes the writer
    /// by value) has returned, and by [`FakeReset`] while it runs.
    struct FakeWriter<'a> {
        written: &'a core::cell::RefCell<std::vec::Vec<u8>>,
    }

    impl embedded_io_async::ErrorType for FakeWriter<'_> {
        type Error = Infallible;
    }

    impl Write for FakeWriter<'_> {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
            self.written.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        async fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    /// A [`WifiControl`] that returns canned outcomes, ignoring the SSID and
    /// password it's given — the parsing/threading of `ssid`/`password` is
    /// exercised separately, this is only about `run_session`'s handling of
    /// the reply.
    struct FakeWifi {
        join_outcome: Result<&'static str, &'static str>,
        forget_outcome: Result<(), &'static str>,
    }

    impl FakeWifi {
        fn joining(join_outcome: Result<&'static str, &'static str>) -> Self {
            FakeWifi {
                join_outcome,
                forget_outcome: Ok(()),
            }
        }
    }

    impl WifiControl for FakeWifi {
        type Address = &'static str;

        async fn join(
            &mut self,
            _ssid: &str,
            _password: &[u8],
        ) -> Result<&'static str, &'static str> {
            self.join_outcome
        }

        async fn forget(&mut self) -> Result<(), &'static str> {
            self.forget_outcome
        }
    }

    /// A [`FactoryReset`] that records, for each request, everything the
    /// Session had written by then — so a test can see the `OK` went first.
    struct FakeReset<'a> {
        written: &'a core::cell::RefCell<std::vec::Vec<u8>>,
        requests: std::vec::Vec<std::string::String>,
    }

    impl FactoryReset for FakeReset<'_> {
        fn request(&mut self) {
            let written = self.written.borrow().clone();
            self.requests
                .push(std::string::String::from_utf8(written).expect("reply is always ASCII"));
        }
    }

    /// A [`WallClock`] stopped at a canned instant, or Unsynced if `None`,
    /// with whatever TZ Rule [`FakeTzStore`] last set (UTC until then).
    struct FakeClock {
        utc: Option<u64>,
        tz: core::cell::RefCell<TzRule>,
    }

    impl FakeClock {
        fn new(utc: Option<u64>) -> Self {
            FakeClock {
                utc,
                tz: core::cell::RefCell::new(TzRule::utc()),
            }
        }
    }

    impl WallClock for FakeClock {
        fn utc_now(&self) -> Option<u64> {
            self.utc
        }

        fn tz_rule(&self) -> TzRule {
            self.tz.borrow().clone()
        }
    }

    /// A [`TzStore`] that puts the rule in force on `clock` when its canned
    /// outcome is `Ok`, standing in for persisting it.
    struct FakeTzStore<'a> {
        clock: &'a FakeClock,
        outcome: Result<(), &'static str>,
    }

    impl TzStore for FakeTzStore<'_> {
        async fn set(&mut self, rule: TzRule) -> Result<(), &'static str> {
            if self.outcome.is_ok() {
                *self.clock.tz.borrow_mut() = rule;
            }
            self.outcome
        }
    }

    /// A [`LinkKeySource`] holding a canned Link Key, or none (Unclaimed).
    struct FakeLinkKey(Option<[u8; LINK_KEY_LEN]>);

    impl LinkKeySource for FakeLinkKey {
        fn link_key(&self) -> Option<[u8; LINK_KEY_LEN]> {
            self.0
        }
    }

    /// A Link Key whose hex form shows the byte order and both nibbles.
    const KEY: [u8; LINK_KEY_LEN] = {
        let mut key = [0u8; LINK_KEY_LEN];
        let mut i = 0;
        while i < LINK_KEY_LEN {
            key[i] = (i as u8) * 8 + 1;
            i += 1;
        }
        key
    };
    const KEY_HEX: &str = "0109111921293139414951596169717981899199a1a9b1b9c1c9d1d9e1e9f1f9";

    /// [`run_bytes`] for input that's valid UTF-8, which is nearly all of it.
    fn run(input: &str, wifi_outcome: Result<&'static str, &'static str>) -> std::string::String {
        run_bytes(input.as_bytes(), wifi_outcome)
    }

    /// Feeds raw `input` bytes through [`run_session`] and returns everything
    /// it wrote back.
    fn run_bytes(input: &[u8], wifi_outcome: Result<&'static str, &'static str>) -> std::string::String {
        run_with_clock(input, wifi_outcome, None)
    }

    /// [`run_bytes`] with the Wall Clock reading `utc` (`None`: Unsynced).
    fn run_with_clock(
        input: &[u8],
        wifi_outcome: Result<&'static str, &'static str>,
        utc: Option<u64>,
    ) -> std::string::String {
        run_with_tz(input, wifi_outcome, utc, Ok(()))
    }

    /// [`run_with_clock`] with `TZ` saves resolving to `tz_outcome`.
    fn run_with_tz(
        input: &[u8],
        wifi_outcome: Result<&'static str, &'static str>,
        utc: Option<u64>,
        tz_outcome: Result<(), &'static str>,
    ) -> std::string::String {
        run_session_with(input, FakeWifi::joining(wifi_outcome), utc, tz_outcome, Transport::Ble, None).0
    }

    /// [`run`] with `FORGET` resolving to `forget_outcome`.
    fn run_forget(input: &str, forget_outcome: Result<(), &'static str>) -> std::string::String {
        let wifi = FakeWifi {
            join_outcome: ok_wifi(),
            forget_outcome,
        };
        run_session_with(input.as_bytes(), wifi, None, Ok(()), Transport::Ble, None).0
    }

    /// [`run`], also returning what had been written at each Factory Reset
    /// request.
    fn run_reset(input: &str) -> (std::string::String, std::vec::Vec<std::string::String>) {
        run_session_with(input.as_bytes(), FakeWifi::joining(ok_wifi()), None, Ok(()), Transport::Ble, None)
    }

    /// [`run`] over `transport`, with `key` as the Link Key.
    fn run_key(input: &str, transport: Transport, key: Option<[u8; LINK_KEY_LEN]>) -> std::string::String {
        run_session_with(input.as_bytes(), FakeWifi::joining(ok_wifi()), None, Ok(()), transport, key).0
    }

    /// Feeds raw `input` bytes through [`run_session`] over `transport`, with
    /// every `WIFI`/`FORGET` handled by `wifi` and `key` as the Link Key, and
    /// returns everything it wrote
    /// back plus [`FakeReset`]'s requests. A fake display task drains
    /// `display` and immediately acks, standing in for `main.rs`'s real
    /// display loop; `select` (rather than `join`) is used since that fake
    /// loop never terminates on its own — only `run_session` reaching a
    /// clean close ends the pair.
    fn run_session_with(
        input: &[u8],
        mut wifi: FakeWifi,
        utc: Option<u64>,
        tz_outcome: Result<(), &'static str>,
        transport: Transport,
        key: Option<[u8; LINK_KEY_LEN]>,
    ) -> (std::string::String, std::vec::Vec<std::string::String>) {
        let reader = FakeReader {
            bytes: input.to_vec(),
            pos: 0,
        };
        let written = core::cell::RefCell::new(std::vec::Vec::new());
        let writer = FakeWriter { written: &written };
        let display = DisplayMailbox::new();
        let mut reset = FakeReset {
            written: &written,
            requests: std::vec::Vec::new(),
        };

        let clock = FakeClock::new(utc);
        let mut tz = FakeTzStore {
            clock: &clock,
            outcome: tz_outcome,
        };

        let key = FakeLinkKey(key);
        let session = run_session(
            reader, writer, transport, &display, &mut wifi, &mut reset, &clock, &mut tz, &key,
        );
        let fake_display = async {
            loop {
                display.receive().await;
                display.ack().await;
            }
        };

        pollster::block_on(select(session, fake_display));
        let replies = std::string::String::from_utf8(written.take()).expect("reply is always ASCII");
        (replies, reset.requests)
    }

    fn ok_wifi() -> Result<&'static str, &'static str> {
        Ok("10.0.0.5")
    }

    #[test]
    fn text_valid() {
        assert_eq!(run("TEXT 12:34\n", ok_wifi()), "OK\n");
    }

    #[test]
    fn text_malformed_wrong_length() {
        assert_eq!(run("TEXT 1234\n", ok_wifi()), "ERR bad args\n");
    }

    #[test]
    fn text_malformed_bad_char() {
        assert_eq!(
            run("TEXT ab:34\n", ok_wifi()),
            "ERR unsupported char\n"
        );
    }

    #[test]
    fn clock_valid() {
        assert_eq!(run("CLOCK\n", ok_wifi()), "OK\n");
    }

    #[test]
    fn color_valid() {
        assert_eq!(run("COLOR ff00aa\n", ok_wifi()), "OK\n");
    }

    #[test]
    fn color_malformed_not_hex() {
        assert_eq!(
            run("COLOR zzzzzz\n", ok_wifi()),
            "ERR bad args\n"
        );
    }

    #[test]
    fn brightness_valid() {
        assert_eq!(run("BRIGHTNESS 200\n", ok_wifi()), "OK\n");
    }

    #[test]
    fn brightness_malformed_out_of_range() {
        assert_eq!(
            run("BRIGHTNESS 999\n", ok_wifi()),
            "ERR bad args\n"
        );
    }

    #[test]
    fn wifi_valid_join_succeeds() {
        assert_eq!(
            run("WIFI myssid mypassword\n", Ok("10.0.0.5")),
            "OK 10.0.0.5\n"
        );
    }

    #[test]
    fn wifi_join_fails() {
        assert_eq!(
            run(
                "WIFI myssid mypassword\n",
                Err("wifi join failed")
            ),
            "ERR wifi join failed\n"
        );
    }

    #[test]
    fn wifi_malformed_missing_password() {
        assert_eq!(run("WIFI myssid\n", ok_wifi()), "ERR bad args\n");
    }

    #[test]
    fn wifi_malformed_ssid_too_long() {
        // 33 bytes: one over the 32-byte SSID bound `parse_wifi` documents.
        let ssid = "a".repeat(33);
        let line = std::format!("WIFI {ssid} mypassword\n");
        assert_eq!(run(&line, ok_wifi()), "ERR bad args\n");
    }

    #[test]
    fn wifi_malformed_password_too_long() {
        // 64 bytes: one over the 63-byte password bound `parse_wifi` documents.
        let password = "a".repeat(64);
        let line = std::format!("WIFI myssid {password}\n");
        assert_eq!(run(&line, ok_wifi()), "ERR bad args\n");
    }

    #[test]
    fn forget_valid() {
        assert_eq!(run_forget("FORGET\n", Ok(())), "OK\n");
    }

    #[test]
    fn forget_is_case_insensitive() {
        assert_eq!(run_forget("forget\n", Ok(())), "OK\n");
    }

    #[test]
    fn forget_clear_fails() {
        assert_eq!(
            run_forget("FORGET\n", Err("flash clear failed")),
            "ERR flash clear failed\n"
        );
    }

    #[test]
    fn reset_replies_ok_then_requests_factory_reset() {
        let (replies, requests) = run_reset("RESET\n");
        assert_eq!(replies, "OK\n");
        assert_eq!(requests, ["OK\n"], "one request, after the OK was written");
    }

    #[test]
    fn reset_is_case_insensitive() {
        assert_eq!(run_reset("reset\n").1.len(), 1);
    }

    #[test]
    fn reset_with_args_is_bad_args_and_does_not_reset() {
        let (replies, requests) = run_reset("RESET now\n");
        assert_eq!(replies, "ERR bad args\n");
        assert!(requests.is_empty());
    }

    #[test]
    fn other_commands_do_not_reset() {
        let (_, requests) = run_reset("CLOCK\nFORGET\nTIME\nBOGUS\n");
        assert!(requests.is_empty());
    }

    #[test]
    fn unpair_is_unknown_and_does_not_reset() {
        let (replies, requests) = run_reset("UNPAIR\n");
        assert_eq!(replies, "ERR unknown command\n");
        assert!(requests.is_empty());
    }

    #[test]
    fn time_synced_replies_iso8601_utc() {
        // 2026-10-03T12:34:56Z.
        assert_eq!(
            run_with_clock(b"TIME\n", ok_wifi(), Some(1_791_030_896)),
            "OK 2026-10-03T12:34:56Z UTC0\n"
        );
    }

    #[test]
    fn time_is_case_insensitive() {
        assert_eq!(
            run_with_clock(b"time\n", ok_wifi(), Some(0)),
            "OK 1970-01-01T00:00:00Z UTC0\n"
        );
    }

    #[test]
    fn time_unsynced() {
        assert_eq!(run("TIME\n", ok_wifi()), "ERR not synced\n");
    }

    #[test]
    fn tz_valid_then_time_reports_it() {
        assert_eq!(
            run_with_clock(
                b"TZ PST8PDT,M3.2.0,M11.1.0\nTIME\n",
                ok_wifi(),
                Some(1_791_030_896)
            ),
            "OK\nOK 2026-10-03T12:34:56Z PST8PDT,M3.2.0,M11.1.0\n"
        );
    }

    #[test]
    fn tz_is_case_insensitive_but_rule_is_kept_verbatim() {
        assert_eq!(
            run_with_clock(b"tz <+0530>-5:30\ntime\n", ok_wifi(), Some(0)),
            "OK\nOK 1970-01-01T00:00:00Z <+0530>-5:30\n"
        );
    }

    #[test]
    fn tz_utc0_restores_utc() {
        assert_eq!(
            run_with_clock(b"TZ EST5EDT\nTZ UTC0\nTIME\n", ok_wifi(), Some(0)),
            "OK\nOK\nOK 1970-01-01T00:00:00Z UTC0\n"
        );
    }

    #[test]
    fn tz_bare_is_bad_args() {
        assert_eq!(run("TZ\n", ok_wifi()), "ERR bad args\n");
        assert_eq!(run("TZ   \n", ok_wifi()), "ERR bad args\n");
    }

    #[test]
    fn tz_malformed_is_bad_tz_and_keeps_rule() {
        assert_eq!(
            run_with_clock(b"TZ PST8PDT,M3.2.0\nTIME\n", ok_wifi(), Some(0)),
            "ERR bad tz\nOK 1970-01-01T00:00:00Z UTC0\n"
        );
    }

    #[test]
    fn tz_day_of_year_form_is_bad_tz() {
        assert_eq!(
            run("TZ <+0330>-3:30<+0430>,J79/24,J263/24\n", ok_wifi()),
            "ERR bad tz\n"
        );
    }

    #[test]
    fn tz_save_fails() {
        assert_eq!(
            run_with_tz(
                b"TZ EST5\nTIME\n",
                ok_wifi(),
                Some(0),
                Err("flash write failed")
            ),
            "ERR flash write failed\nOK 1970-01-01T00:00:00Z UTC0\n"
        );
    }

    #[test]
    fn time_reply_with_longest_tz_fits() {
        let rule = std::format!("<{}>0", "A".repeat(crate::tz::MAX_TZ_LEN - 3));
        let input = std::format!("TZ {rule}\nTIME\n");
        let expected = std::format!("OK\nOK 2026-10-03T12:34:56Z {rule}\n");
        assert_eq!(expected.len() - 3, MAX_REPLY_LEN);
        assert_eq!(
            run_with_clock(input.as_bytes(), ok_wifi(), Some(1_791_030_896)),
            expected
        );
    }

    #[test]
    fn key_over_ble_replies_hex_link_key() {
        assert_eq!(
            run_key("KEY\n", Transport::Ble, Some(KEY)),
            std::format!("OK {KEY_HEX}\n")
        );
    }

    #[test]
    fn key_reply_fits() {
        assert!(std::format!("OK {KEY_HEX}\n").len() <= MAX_REPLY_LEN);
    }

    #[test]
    fn key_is_case_insensitive() {
        assert_eq!(
            run_key("key\n", Transport::Ble, Some(KEY)),
            std::format!("OK {KEY_HEX}\n")
        );
    }

    #[test]
    fn key_with_args_is_bad_args() {
        assert_eq!(run_key("KEY please\n", Transport::Ble, Some(KEY)), "ERR bad args\n");
    }

    #[test]
    fn key_unclaimed_is_not_claimed() {
        assert_eq!(run_key("KEY\n", Transport::Ble, None), "ERR not claimed\n");
    }

    #[test]
    fn key_off_ble_is_not_allowed() {
        assert_eq!(run_key("KEY\n", Transport::Usb, Some(KEY)), "ERR not allowed\n");
    }

    #[test]
    fn key_off_ble_with_args_is_still_bad_args() {
        // Parse errors come first, whatever the Transport.
        assert_eq!(run_key("KEY x\n", Transport::Usb, Some(KEY)), "ERR bad args\n");
    }

    #[test]
    fn other_commands_work_off_ble() {
        assert_eq!(
            run_key("CLOCK\nKEY\nTZ EST5\n", Transport::Usb, Some(KEY)),
            "OK\nERR not allowed\nOK\n"
        );
    }

    #[test]
    fn unknown_command() {
        assert_eq!(run("BOGUS\n", ok_wifi()), "ERR unknown command\n");
    }

    #[test]
    fn non_ascii_color_args() {
        assert_eq!(
            run_bytes(b"COLOR a\xe9\xe9b\n", ok_wifi()),
            "ERR unsupported char\n"
        );
    }

    #[test]
    fn non_ascii_in_first_four_chars() {
        // Within the prefix `run_session` checks for `WIFI` before parsing.
        assert_eq!(
            run_bytes(b"abc\xe9\n", ok_wifi()),
            "ERR unsupported char\n"
        );
    }

    #[test]
    fn non_ascii_line_then_valid_command() {
        assert_eq!(
            run_bytes(b"abc\xe9\nCLOCK\n", ok_wifi()),
            "ERR unsupported char\nOK\n"
        );
    }

    #[test]
    fn non_ascii_wifi_never_joins() {
        // The canned `Ok` join outcome would reply `OK 10.0.0.5` if the line
        // were parsed and dispatched.
        assert_eq!(
            run_bytes(b"WIFI net p\xe9ss\n", ok_wifi()),
            "ERR unsupported char\n"
        );
    }

    #[test]
    fn non_ascii_only_line() {
        assert_eq!(
            run_bytes(b"\xe9\xe9\nCLOCK\n", ok_wifi()),
            "ERR unsupported char\nOK\n"
        );
    }

    #[test]
    fn line_too_long_counting_non_ascii() {
        // 120 raw bytes: too long, even though only 60 are storable ASCII.
        let mut line = std::vec![b'9'; 60];
        line.extend(core::iter::repeat_n(0xe9, 60));
        line.push(b'\n');
        assert_eq!(
            run_bytes(&line, ok_wifi()),
            "ERR line too long\n"
        );
    }

    #[test]
    fn line_too_long_with_non_ascii() {
        let mut line = b"TEXT \xe9".to_vec();
        line.extend(core::iter::repeat_n(b'9', 200));
        line.push(b'\n');
        assert_eq!(
            run_bytes(&line, ok_wifi()),
            "ERR line too long\n"
        );
    }

    #[test]
    fn line_too_long() {
        let long_line = "TEXT ".to_string() + &"9".repeat(200) + "\n";
        assert_eq!(run(&long_line, ok_wifi()), "ERR line too long\n");
    }

    // Concurrent Sessions: two `run_session`s and a fake display loop that
    // takes several polls to draw, all joined in one future the way
    // `main.rs` joins the USB and BLE Sessions with the real display loop.

    const USB: usize = 0;
    const BLE: usize = 1;

    #[derive(Debug, Clone, PartialEq)]
    enum Event {
        /// The display finished drawing this `TEXT` value.
        Shown(std::string::String),
        /// This Session wrote an `OK` reply.
        Replied(usize),
        /// This Session's `run_session` returned.
        Ended(usize),
    }

    type Log = core::cell::RefCell<std::vec::Vec<Event>>;

    /// One Session's client: which `TEXT` values it sends, how many polls it
    /// waits before its first byte, and whether its Transport fails on
    /// write.
    struct Client {
        texts: std::vec::Vec<std::string::String>,
        start_delay: usize,
        fail_writes: bool,
    }

    impl Client {
        /// `count` distinct `TEXT` values, unique to `session`.
        fn new(session: usize, count: usize, start_delay: usize) -> Self {
            Client {
                texts: (0..count).map(|k| std::format!("{session}{k}:00")).collect(),
                start_delay,
                fail_writes: false,
            }
        }
    }

    /// Like [`FakeReader`], but yields `pending` polls before serving the
    /// next byte, so a client can arrive late, and pauses one poll after
    /// each line as any real client does. That pause matters: the turn lock
    /// isn't FIFO, so a client sending with no gap at all could keep
    /// re-taking it before the other Session is polled.
    struct SlowReader {
        bytes: std::vec::Vec<u8>,
        pos: usize,
        pending: usize,
    }

    impl embedded_io_async::ErrorType for SlowReader {
        type Error = Infallible;
    }

    impl Read for SlowReader {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
            while self.pending > 0 {
                self.pending -= 1;
                embassy_futures::yield_now().await;
            }
            if self.pos >= self.bytes.len() {
                return Ok(0);
            }
            buf[0] = self.bytes[self.pos];
            self.pos += 1;
            if buf[0] == b'\n' {
                self.pending = 1;
            }
            Ok(1)
        }
    }

    /// Logs each `OK` its Session writes, or fails every write.
    struct LoggingWriter<'a> {
        session: usize,
        log: &'a Log,
        fail: bool,
    }

    impl embedded_io_async::ErrorType for LoggingWriter<'_> {
        type Error = embedded_io_async::ErrorKind;
    }

    impl Write for LoggingWriter<'_> {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
            if self.fail {
                return Err(embedded_io_async::ErrorKind::BrokenPipe);
            }
            assert_eq!(buf, b"OK\n", "display Commands only ever reply OK");
            self.log.borrow_mut().push(Event::Replied(self.session));
            Ok(buf.len())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Polls `fut` to completion, but only when something woke it: a future
    /// left pending with no wake outstanding is hung and fails the test,
    /// rather than blocking forever like `pollster` would.
    fn block_on_or_hang<F: core::future::Future>(fut: F) -> F::Output {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        struct Woken(AtomicBool);
        impl Wake for Woken {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let woken = Arc::new(Woken(AtomicBool::new(true)));
        let waker = Waker::from(woken.clone());
        let mut cx = Context::from_waker(&waker);
        let mut fut = core::pin::pin!(fut);
        for _ in 0..1_000_000 {
            assert!(woken.0.swap(false, Ordering::SeqCst), "hung: pending with no wake");
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
        }
        panic!("hung: poll budget exhausted");
    }

    /// Runs the USB and BLE clients' Sessions (USB polled first, as in
    /// `main.rs`) against a fake display loop that yields `draw_polls` times
    /// per Command, until both Sessions end. Returns everything that
    /// happened, in order.
    fn run_pair(usb: Client, ble: Client, draw_polls: usize) -> std::vec::Vec<Event> {
        let log = Log::default();
        let display = DisplayMailbox::new();

        let session = |id: usize, client: Client| {
            let (log, display) = (&log, &display);
            async move {
                let mut bytes = std::vec::Vec::new();
                for text in &client.texts {
                    bytes.extend_from_slice(std::format!("TEXT {text}\n").as_bytes());
                }
                let reader = SlowReader {
                    bytes,
                    pos: 0,
                    pending: client.start_delay,
                };
                let writer = LoggingWriter {
                    session: id,
                    log,
                    fail: client.fail_writes,
                };
                let mut wifi = FakeWifi::joining(ok_wifi());
                let unused = core::cell::RefCell::default();
                let mut reset = FakeReset {
                    written: &unused,
                    requests: std::vec::Vec::new(),
                };
                let clock = FakeClock::new(None);
                let mut tz = FakeTzStore {
                    clock: &clock,
                    outcome: Ok(()),
                };
                run_session(
                    reader,
                    writer,
                    Transport::Ble,
                    display,
                    &mut wifi,
                    &mut reset,
                    &clock,
                    &mut tz,
                    &FakeLinkKey(None),
                )
                .await;
                log.borrow_mut().push(Event::Ended(id));
            }
        };

        let fake_display = async {
            loop {
                let Command::Text(text) = display.receive().await else {
                    panic!("only TEXT is sent here");
                };
                for _ in 0..draw_polls {
                    embassy_futures::yield_now().await;
                }
                log.borrow_mut().push(Event::Shown(text.as_str().into()));
                display.ack().await;
            }
        };

        let sessions = embassy_futures::join::join(session(USB, usb), session(BLE, ble));
        block_on_or_hang(select(sessions, fake_display));
        log.into_inner()
    }

    /// Where `event` appears in `log`.
    fn positions(log: &[Event], event: &Event) -> std::vec::Vec<usize> {
        log.iter()
            .enumerate()
            .filter(|(_, e)| *e == event)
            .map(|(i, _)| i)
            .collect()
    }

    /// Asserts each of `session`'s `texts` got exactly one `OK`, and that
    /// the k-th `OK` came after the display showed the k-th text.
    fn assert_replies_follow_own_commands(log: &[Event], session: usize, texts: &[std::string::String]) {
        let replies = positions(log, &Event::Replied(session));
        assert_eq!(replies.len(), texts.len(), "session {session}: one reply per Command\n{log:#?}");
        for (text, &reply_at) in texts.iter().zip(&replies) {
            let shown = positions(log, &Event::Shown(text.clone()));
            assert_eq!(shown.len(), 1, "{text} shown exactly once\n{log:#?}");
            assert!(
                shown[0] < reply_at,
                "session {session} replied OK before {text} was shown\n{log:#?}"
            );
        }
    }

    fn assert_pair_ok(usb: Client, ble: Client, draw_polls: usize) {
        let (usb_texts, ble_texts) = (usb.texts.clone(), ble.texts.clone());
        let log = run_pair(usb, ble, draw_polls);
        assert_replies_follow_own_commands(&log, USB, &usb_texts);
        assert_replies_follow_own_commands(&log, BLE, &ble_texts);
    }

    #[test]
    fn concurrent_usb_first_then_ble() {
        assert_pair_ok(Client::new(USB, 1, 0), Client::new(BLE, 1, 1), 3);
    }

    #[test]
    fn concurrent_ble_first_then_usb_mid_frame() {
        assert_pair_ok(Client::new(USB, 1, 1), Client::new(BLE, 1, 0), 3);
    }

    #[test]
    fn concurrent_sweep_start_delays() {
        for draw_polls in [0, 1, 3] {
            for usb_delay in 0..8 {
                for ble_delay in 0..8 {
                    assert_pair_ok(
                        Client::new(USB, 3, usb_delay),
                        Client::new(BLE, 3, ble_delay),
                        draw_polls,
                    );
                }
            }
        }
    }

    #[test]
    fn failed_transport_waiting_for_turn_ends_and_other_session_continues() {
        let usb = Client::new(USB, 5, 0);
        let usb_texts = usb.texts.clone();
        let mut ble = Client::new(BLE, 3, 1);
        ble.fail_writes = true;

        let log = run_pair(usb, ble, 3);

        // BLE's first Command arrives while USB's is being drawn: it may wait
        // out that frame and draw its own, but no more.
        let ended_at = log
            .iter()
            .position(|e| *e == Event::Ended(BLE))
            .expect("BLE session ended");
        let frames_before_end = log[..ended_at]
            .iter()
            .filter(|e| matches!(e, Event::Shown(_)))
            .count();
        assert!(frames_before_end <= 2, "BLE outlived its turn\n{log:#?}");
        assert!(!log.contains(&Event::Replied(BLE)));
        assert_replies_follow_own_commands(&log, USB, &usb_texts);
    }
}
