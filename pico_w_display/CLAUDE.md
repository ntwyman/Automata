# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Bare-metal Rust firmware for a Raspberry Pi Pico 2 (RP2350) that displays elapsed time in `mm:ss` format on a 17×17 WS2812 LED matrix. Uses the Embassy async embedded framework — no OS, no std.

## Build & Flash

```bash
# Build (cross-compiles to thumbv8m.main-none-eabihf automatically via .cargo/config.toml)
cargo build
1
# Flash via picotool (board must be in BOOTSEL/USB mode)
cargo run

# Flash via probe-rs (SWD debug probe attached)
probe-rs run --chip RP2350 target/thumbv8m.main-none-eabihf/debug/pico_w_display

# Monitor defmt RTT logs (requires probe-rs)
probe-rs attach --chip RP2350
```

Toolchain is pinned to Rust 1.92 in [rust-toolchain.toml](rust-toolchain.toml). Flashing tool defaults to `picotool`; the `probe-rs` runner is commented out in [.cargo/config.toml](.cargo/config.toml).

The VSCode launch config ([.vscode/launch.json](.vscode/launch.json)) uses `probe-rs-debug` for flash + RTT debugging.

## Architecture

```
src/
  main.rs   — Embassy async main: initialises PIO/WS2812, runs the mm:ss display loop
  grid.rs   — Grid<WIDTH, SIZE> abstraction over a WS2812 LED strip wired as a 2-D matrix
  fonts.rs  — Glyph trait + bitmap font data for digits 0-9 and the colon separator
```

### Key design points

**Grid coordinate model** — `Grid` is generic over `WIDTH` and `SIZE` (total LEDs). The `GridOrigin` enum maps logical `(x, y)` to the physical strip index, accounting for how the LED strip snakes through the matrix. Currently wired as `TopRight`.

**Glyph rendering** — Glyphs are column-major bitmaps. Each column is one `u32`; bit `n` = row `n`. `blit_glyph` clears the bounding rect first, then paints foreground pixels. Adding new characters means implementing the `Glyph` trait or adding a new `GlyphNbyM` struct + const.

**Display layout** — 17×17 grid, digits are 3 wide × 6 tall, vertically centred at `y=5`. Column offsets: tens-of-minutes=0, units-of-minutes=4, colon=8, tens-of-seconds=10, units-of-seconds=14.

**Embassy async** — The main loop polls `Ticker::every(100 ms)` and only redraws when seconds change. `grid.update()` is `async` because WS2812 output is DMA-driven through a PIO state machine.

**`no_std` / `no_main`** — Standard library is unavailable. Panics print via `defmt` RTT and halt. Use `defmt::info!/warn!/error!` for logging; `DEFMT_LOG=debug` is set in `.cargo/config.toml`.
