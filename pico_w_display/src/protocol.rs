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

use core::fmt::Write as _;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embedded_io_async::{Read, Write};
use heapless::String;
use smart_leds::RGB8;

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
/// without depending on any concrete Wi-Fi stack. Kept separate from
/// [`Command`]/[`CommandChannel`] because joining a network is a direct,
/// awaited round-trip — there's no display to ack it.
// Only implemented in this workspace (by `wifi::Wifi` and this module's own
// test fake), so the usual reason for the `async_fn_in_trait` lint — an
// external implementor needing `Send` bounds it can't add later — doesn't
// apply.
#[allow(async_fn_in_trait)]
pub trait WifiJoin {
    /// The joined network's address, formatted straight into the `OK <addr>`
    /// reply — hence the `Display` bound.
    type Address: core::fmt::Display;

    /// Joins `ssid` using `password`, returning the assigned address, or a
    /// reason string that's safe to send straight back to a client as
    /// `ERR <reason>`.
    async fn join(&mut self, ssid: &str, password: &[u8]) -> Result<Self::Address, &'static str>;
}

/// Longest line we'll accept; comfortably longer than any real command (the
/// longest is `WIFI <32-byte ssid> <63-byte password>`). Anything longer is
/// rejected as `ERR line too long` rather than silently truncated.
const MAX_LINE_LEN: usize = 104;

/// A single-slot mailbox from the protocol session to the display loop.
pub type CommandChannel = Channel<CriticalSectionRawMutex, Command, 1>;
/// A single-slot mailbox the display loop uses to acknowledge it applied a
/// command, so the protocol session knows when it's safe to reply `OK`.
pub type AckChannel = Channel<CriticalSectionRawMutex, (), 1>;

/// A parsed, fully-validated client command. By the time one of these exists,
/// it is safe to apply directly to the display with no further checks.
pub enum Command {
    /// Render this exact 5-character `DD:DD`-shaped string with the existing
    /// digit/colon font, replacing the clock until [`Command::Clock`] is sent.
    Text(String<5>),
    /// Resume the automatic elapsed-time clock display.
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
    Wifi(String<32>, String<63>),
}

#[derive(defmt::Format)]
enum ProtocolError {
    UnknownCommand,
    BadArgs,
    UnsupportedChar,
}

impl ProtocolError {
    fn reason(&self) -> &'static str {
        match self {
            ProtocolError::UnknownCommand => "unknown command",
            ProtocolError::BadArgs => "bad args",
            ProtocolError::UnsupportedChar => "unsupported char",
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

    let mut ssid_buf: String<32> = String::new();
    ssid_buf
        .push_str(ssid)
        .map_err(|_| ProtocolError::BadArgs)?;
    let mut password_buf: String<63> = String::new();
    password_buf
        .push_str(password)
        .map_err(|_| ProtocolError::BadArgs)?;
    Ok(ParsedLine::Wifi(ssid_buf, password_buf))
}

/// Outcome of reading one line from the transport.
enum Line {
    /// A complete line, ready to parse.
    Ready,
    /// The line exceeded [`MAX_LINE_LEN`]; bytes up to the next `\n` were
    /// discarded to resynchronize, and no attempt was made to parse it.
    TooLong,
}

/// Reads bytes until a `\r` or `\n`, appending them into `buf` (cleared by
/// the caller first). A terminator seen while `buf` is still empty is just a
/// blank line — most commonly the second half of a `\r\n`/`\n\r` pair — and
/// is skipped rather than reported, so CR-only, LF-only and CRLF senders all
/// work. Returns `Err(())` on any transport error, or a clean close (`read`
/// == 0 bytes) — both mean the session is over.
async fn read_line<R: Read>(reader: &mut R, buf: &mut String<MAX_LINE_LEN>) -> Result<Line, ()> {
    let mut overflowed = false;
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).await.map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        if byte[0] == b'\r' || byte[0] == b'\n' {
            if buf.is_empty() && !overflowed {
                continue;
            }
            return Ok(if overflowed {
                Line::TooLong
            } else {
                Line::Ready
            });
        }
        if !overflowed && buf.push(byte[0] as char).is_err() {
            overflowed = true;
        }
    }
}

/// Runs one client session to completion: reads lines, dispatches parsed
/// commands to the display loop over `commands`, waits for `acks` to confirm
/// the display applied it, and writes back `OK`/`ERR`. Returns once the
/// transport errors or closes (e.g. USB disconnect), so the caller can wait
/// for a new connection and call this again.
pub async fn run_session<R: Read, W: Write, J: WifiJoin>(
    mut reader: R,
    mut writer: W,
    commands: &CommandChannel,
    acks: &AckChannel,
    wifi: &mut J,
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

        let mut reply: String<40> = String::new();
        match outcome {
            Line::TooLong => {
                let _ = reply.push_str("ERR line too long\n");
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
                        commands.send(command).await;
                        acks.receive().await;
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
                    Err(e) => {
                        let _ = writeln!(reply, "ERR {}", e.reason());
                    }
                }
            }
        }

        if writer.write_all(reply.as_bytes()).await.is_err() {
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
    /// by value) has returned.
    struct FakeWriter<'a> {
        written: &'a mut std::vec::Vec<u8>,
    }

    impl embedded_io_async::ErrorType for FakeWriter<'_> {
        type Error = Infallible;
    }

    impl Write for FakeWriter<'_> {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        async fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    /// A [`WifiJoin`] that returns a canned outcome, ignoring the credentials
    /// it's given — the parsing/threading of `ssid`/`password` is exercised
    /// separately, this is only about `run_session`'s handling of the reply.
    struct FakeWifi {
        outcome: Result<&'static str, &'static str>,
    }

    impl WifiJoin for FakeWifi {
        type Address = &'static str;

        async fn join(
            &mut self,
            _ssid: &str,
            _password: &[u8],
        ) -> Result<&'static str, &'static str> {
            self.outcome
        }
    }

    /// Feeds `input` through [`run_session`] and returns everything it wrote
    /// back. A fake display task drains `commands` and immediately acks,
    /// standing in for `main.rs`'s real display loop; `select` (rather than
    /// `join`) is used since that fake loop never terminates on its own —
    /// only `run_session` reaching a clean close ends the pair.
    fn run(input: &str, wifi_outcome: Result<&'static str, &'static str>) -> std::string::String {
        let reader = FakeReader {
            bytes: input.as_bytes().to_vec(),
            pos: 0,
        };
        let mut written = std::vec::Vec::new();
        let writer = FakeWriter {
            written: &mut written,
        };
        let commands = CommandChannel::new();
        let acks = AckChannel::new();
        let mut wifi = FakeWifi {
            outcome: wifi_outcome,
        };

        let session = run_session(reader, writer, &commands, &acks, &mut wifi);
        let fake_display = async {
            loop {
                commands.receive().await;
                acks.send(()).await;
            }
        };

        pollster::block_on(select(session, fake_display));
        std::string::String::from_utf8(written).expect("reply is always ASCII")
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
        assert_eq!(run("TEXT ab:34\n", ok_wifi()), "ERR unsupported char\n");
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
        assert_eq!(run("COLOR zzzzzz\n", ok_wifi()), "ERR bad args\n");
    }

    #[test]
    fn brightness_valid() {
        assert_eq!(run("BRIGHTNESS 200\n", ok_wifi()), "OK\n");
    }

    #[test]
    fn brightness_malformed_out_of_range() {
        assert_eq!(run("BRIGHTNESS 999\n", ok_wifi()), "ERR bad args\n");
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
            run("WIFI myssid mypassword\n", Err("wifi join failed")),
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
    fn unknown_command() {
        assert_eq!(run("BOGUS\n", ok_wifi()), "ERR unknown command\n");
    }

    #[test]
    fn line_too_long() {
        let long_line = "TEXT ".to_string() + &"9".repeat(200) + "\n";
        assert_eq!(run(&long_line, ok_wifi()), "ERR line too long\n");
    }
}
