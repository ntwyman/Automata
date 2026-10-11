//! Renders the Analog Face to PNGs on the host, to judge how it looks
//! without flashing the device:
//!
//! ```sh
//! cargo run --example analog_png --target aarch64-apple-darwin --profile host-test [-- <dir>]
//! ```
//!
//! Writes one PNG per sample time (`HHMMSS.png`, or `unsynced.png`) in
//! white, plus `sheet.png`: every sample in a row, once in white and once
//! in the boot colour. `<dir>` defaults to `target/analog_png`. Each LED is
//! drawn as a dot on black, scaled up, so the image reads like the grid.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use pico_w_display::analog::{AnalogFrame, SIZE, render_analog, render_analog_unsynced};
use smart_leds::RGB8;

/// Pixels per LED in the output, and the dot's radius within that cell.
const CELL: usize = 24;
const DOT_RADIUS: f32 = 9.5;
/// Gap between faces on the sheet, in pixels.
const GAP: usize = CELL;

const WHITE: RGB8 = RGB8 { r: 255, g: 255, b: 255 };
/// `main.rs`'s boot foreground (`smart_leds::colors::DARK_BLUE`).
const BOOT: RGB8 = RGB8 { r: 0, g: 0, b: 139 };
const BLACK: RGB8 = RGB8 { r: 0, g: 0, b: 0 };

/// The sample times, `None` for Unsynced.
const SAMPLES: [Option<(u8, u8, u8)>; 9] = [
    Some((12, 0, 0)),
    Some((3, 0, 0)),
    Some((3, 15, 30)),
    Some((6, 30, 0)),
    Some((9, 45, 15)),
    Some((10, 10, 40)),
    Some((1, 5, 20)),
    Some((11, 59, 59)),
    None,
];

fn main() {
    let dir: PathBuf = std::env::args_os().nth(1).map(Into::into).unwrap_or_else(|| "target/analog_png".into());
    std::fs::create_dir_all(&dir).expect("create output dir");

    let frame = |sample: Option<(u8, u8, u8)>, fg| match sample {
        Some((h, m, s)) => render_analog(h, m, s, fg, BLACK),
        None => render_analog_unsynced(fg, BLACK),
    };

    for sample in SAMPLES {
        let name = match sample {
            Some((h, m, s)) => format!("{h:02}{m:02}{s:02}.png"),
            None => "unsynced.png".into(),
        };
        let image = Image::faces(&[vec![frame(sample, WHITE)]]);
        image.save(&dir.join(name));
    }

    let rows: Vec<Vec<AnalogFrame>> = [WHITE, BOOT]
        .into_iter()
        .map(|fg| SAMPLES.iter().map(|&sample| frame(sample, fg)).collect())
        .collect();
    let sheet = dir.join("sheet.png");
    Image::faces(&rows).save(&sheet);
    println!("wrote {} faces and {}", SAMPLES.len(), sheet.display());
}

/// An RGB image being drawn.
struct Image {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Image {
    /// `rows` of faces laid out in a grid, `GAP` apart and around.
    fn faces(rows: &[Vec<AnalogFrame>]) -> Self {
        let face = SIZE * CELL;
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut image = Image {
            width: GAP + cols * (face + GAP),
            height: GAP + rows.len() * (face + GAP),
            pixels: Vec::new(),
        };
        image.pixels = vec![0; image.width * image.height * 3];
        for (row, frames) in rows.iter().enumerate() {
            for (col, frame) in frames.iter().enumerate() {
                image.draw_face(GAP + col * (face + GAP), GAP + row * (face + GAP), frame);
            }
        }
        image
    }

    /// Draws `frame` with its top left at (`left`, `top`): each LED a dot,
    /// unlit ones faintly so the grid stays visible.
    fn draw_face(&mut self, left: usize, top: usize, frame: &AnalogFrame) {
        for (i, &led) in frame.iter().enumerate() {
            let shown = if led == BLACK { RGB8::new(24, 24, 24) } else { led };
            let (cx, cy) = (left + (i % SIZE) * CELL, top + (i / SIZE) * CELL);
            for y in 0..CELL {
                for x in 0..CELL {
                    let (dx, dy) = (x as f32 + 0.5 - CELL as f32 / 2.0, y as f32 + 0.5 - CELL as f32 / 2.0);
                    if dx * dx + dy * dy <= DOT_RADIUS * DOT_RADIUS {
                        let at = ((cy + y) * self.width + cx + x) * 3;
                        self.pixels[at..at + 3].copy_from_slice(&[shown.r, shown.g, shown.b]);
                    }
                }
            }
        }
    }

    fn save(&self, path: &Path) {
        let file = BufWriter::new(File::create(path).expect("create PNG"));
        let mut encoder = png::Encoder::new(file, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("write PNG header");
        writer.write_image_data(&self.pixels).expect("write PNG data");
    }
}
