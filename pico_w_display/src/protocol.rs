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

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embedded_io_async::{Read, Write};
use heapless::String;
use smart_leds::RGB8;

/// Longest line we'll accept; comfortably longer than any real command
/// (the longest is `BRIGHTNESS 255`). Anything longer is rejected as
/// `ERR line too long` rather than silently truncated.
const MAX_LINE_LEN: usize = 32;

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
fn parse_line(line: &str) -> Result<Command, ProtocolError> {
    let line = line.trim();
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

    if cmd.eq_ignore_ascii_case("TEXT") {
        parse_text(rest)
    } else if cmd.eq_ignore_ascii_case("CLOCK") {
        Ok(Command::Clock)
    } else if cmd.eq_ignore_ascii_case("COLOR") {
        parse_color(rest)
    } else if cmd.eq_ignore_ascii_case("BRIGHTNESS") {
        parse_brightness(rest)
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
pub async fn run_session<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    commands: &CommandChannel,
    acks: &AckChannel,
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
                info!("recv: {}", line.as_str());
                match parse_line(&line) {
                    Ok(command) => {
                        commands.send(command).await;
                        acks.receive().await;
                        let _ = reply.push_str("OK\n");
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
