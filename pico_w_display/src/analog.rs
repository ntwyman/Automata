//! The Analog Face (`CONTEXT.md`'s Face entry): the Wall Clock drawn as
//! hands on the 17×17 grid, centred on (8, 8) with radius 8.
//!
//! Frames are row-major, `y * 17 + x`, with (0, 0) top left; `main.rs`'s
//! grid maps that onto the LED chain.

use smart_leds::RGB8;

/// The grid's width and height, in LEDs.
pub const SIZE: usize = 17;

/// One frame: every LED's colour, row-major.
pub type AnalogFrame = [RGB8; SIZE * SIZE];

/// The centre pixel's coordinate on both axes, and the rim's radius.
const CENTRE: f32 = 8.0;
const RIM: f32 = 8.0;

/// Hand lengths, centre to tip, in pixels.
const HOUR_LEN: f32 = 5.0;
const MINUTE_LEN: f32 = 7.0;

/// Shares of `fg` each element is drawn at: the minute hand is `fg` itself.
const HOUR_SHADE: f32 = 0.5;
const MARKER_SHADE: f32 = 0.3;
const SECONDS_SHADE: f32 = 0.6;

/// The Analog Face at `h:m:s` (24-hour `h`): hour and minute hands in
/// shades of `fg`, rim markers at 12/3/6/9, and a seconds pixel orbiting
/// the rim, over `bg`. The hands sweep: the hour hand moves with the
/// minutes and the minute hand with the seconds.
pub fn render_analog(h: u8, m: u8, s: u8, fg: RGB8, bg: RGB8) -> AnalogFrame {
    let mut frame = render_analog_unsynced(fg, bg);
    let seconds = s as f32;
    let minutes = m as f32 + seconds / 60.0;
    let hours = (h % 12) as f32 + minutes / 60.0;
    draw_hand(&mut frame, hours / 12.0, HOUR_LEN, shade(fg, HOUR_SHADE));
    draw_hand(&mut frame, minutes / 60.0, MINUTE_LEN, fg);

    let (dx, dy) = direction(seconds / 60.0);
    let x = libm::roundf(CENTRE + RIM * dx) as usize;
    let y = libm::roundf(CENTRE + RIM * dy) as usize;
    frame[y * SIZE + x] = shade(fg, SECONDS_SHADE);
    frame
}

/// The Unsynced Analog Face: rim markers only, no hands or seconds pixel —
/// the analog `--:--`.
pub fn render_analog_unsynced(fg: RGB8, bg: RGB8) -> AnalogFrame {
    let mut frame = [bg; SIZE * SIZE];
    let marker = shade(fg, MARKER_SHADE);
    for (x, y) in [(8, 0), (16, 8), (8, 16), (0, 8)] {
        frame[y * SIZE + x] = marker;
    }
    frame
}

/// The unit vector pointing `turns` of the way clockwise round from 12, in
/// grid coordinates (y grows downward).
fn direction(turns: f32) -> (f32, f32) {
    let angle = turns * 2.0 * core::f32::consts::PI;
    (libm::sinf(angle), -libm::cosf(angle))
}

/// Draws a 1 px hand from the centre `len` pixels towards `turns`,
/// anti-aliased: each pixel takes `color` in proportion to how much of it
/// the hand covers, judged by its centre's distance from the hand.
fn draw_hand(frame: &mut AnalogFrame, turns: f32, len: f32, color: RGB8) {
    let (dx, dy) = direction(turns);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 - CENTRE, y as f32 - CENTRE);
            // Nearest point on the hand, as a distance along it.
            let along = (px * dx + py * dy).clamp(0.0, len);
            let distance = libm::hypotf(px - along * dx, py - along * dy);
            let coverage = (1.0 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let i = y * SIZE + x;
                frame[i] = mix(frame[i], color, coverage);
            }
        }
    }
}

/// `color` with each channel scaled by `share`.
fn shade(color: RGB8, share: f32) -> RGB8 {
    mix(RGB8::default(), color, share)
}

/// `from` moved `share` of the way to `to`, per channel.
fn mix(from: RGB8, to: RGB8, share: f32) -> RGB8 {
    let channel = |a: u8, b: u8| libm::roundf(a as f32 + (b as f32 - a as f32) * share) as u8;
    RGB8::new(channel(from.r, to.r), channel(from.g, to.g), channel(from.b, to.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: RGB8 = RGB8 { r: 255, g: 255, b: 255 };
    const BG: RGB8 = RGB8 { r: 0, g: 0, b: 0 };
    const MARKERS: [(usize, usize); 4] = [(8, 0), (16, 8), (8, 16), (0, 8)];

    fn at(frame: &AnalogFrame, x: usize, y: usize) -> RGB8 {
        frame[y * SIZE + x]
    }

    /// Every (x, y) not drawn in the background colour.
    fn lit(frame: &AnalogFrame) -> std::vec::Vec<(usize, usize)> {
        (0..SIZE * SIZE)
            .filter(|&i| frame[i] != BG)
            .map(|i| (i % SIZE, i / SIZE))
            .collect()
    }

    /// [`lit`], less the rim markers.
    fn lit_off_markers(frame: &AnalogFrame) -> std::vec::Vec<(usize, usize)> {
        lit(frame).into_iter().filter(|p| !MARKERS.contains(p)).collect()
    }

    fn brightness(c: RGB8) -> u16 {
        c.r as u16 + c.g as u16 + c.b as u16
    }

    #[test]
    fn centre_pixel_is_lit() {
        for (h, m, s) in [(0, 0, 0), (3, 15, 0), (6, 30, 0), (9, 45, 30), (11, 59, 59)] {
            assert_ne!(at(&render_analog(h, m, s, FG, BG), 8, 8), BG, "{h}:{m}:{s}");
        }
    }

    #[test]
    fn at_twelve_the_hands_overlap_pointing_up() {
        // The seconds pixel is on the 12 marker, so every lit pixel is a
        // marker or on the column above the centre.
        let frame = render_analog(12, 0, 0, FG, BG);
        for (x, y) in lit_off_markers(&frame) {
            assert!(x == 8 && y <= 8, "({x}, {y}) lit");
        }
        assert_eq!(at(&frame, 8, 1), FG, "the minute hand reaches near the rim");
    }

    #[test]
    fn at_three_the_minute_hand_points_up_and_the_hour_hand_right() {
        let frame = render_analog(3, 0, 0, FG, BG);
        assert_eq!(at(&frame, 8, 2), FG, "minute hand, full colour");
        let hour = at(&frame, 12, 8);
        assert!(hour != BG && brightness(hour) < brightness(FG), "hour hand, dimmer: {hour:?}");
        assert_eq!(at(&frame, 8, 12), BG);
        assert_eq!(at(&frame, 4, 8), BG);
    }

    #[test]
    fn at_six_thirty_both_hands_point_down() {
        let frame = render_analog(18, 30, 0, FG, BG);
        assert_eq!(at(&frame, 8, 14), FG, "minute hand straight down");
        // The seconds pixel is on the 12 marker; the hour hand is just past
        // the 6, so nothing above the centre row is lit.
        for (x, y) in lit_off_markers(&frame) {
            assert!(y >= 8, "({x}, {y}) lit");
        }
    }

    fn radius(x: usize, y: usize) -> f32 {
        ((x as f32 - 8.0).powi(2) + (y as f32 - 8.0).powi(2)).sqrt()
    }

    #[test]
    fn the_seconds_pixel_is_on_the_rim() {
        // Near 12:00 the 7 px minute hand lights nothing more than 7.5 px
        // out, so whatever is, off the markers, is the seconds pixel.
        for s in [5, 10, 20, 25, 35, 40, 50, 55] {
            let frame = render_analog(0, 0, s, FG, BG);
            let outer: std::vec::Vec<_> =
                lit_off_markers(&frame).into_iter().filter(|&(x, y)| radius(x, y) > 7.5).collect();
            assert_eq!(outer.len(), 1, "s={s}: {outer:?}");
            let (x, y) = outer[0];
            let r = radius(x, y);
            assert!((r - 8.0).abs() <= 0.75, "s={s}: ({x}, {y}) is {r} from the centre");
        }
    }

    #[test]
    fn the_unsynced_frame_has_markers_only() {
        let mut lit = lit(&render_analog_unsynced(FG, BG));
        lit.sort();
        let mut markers = MARKERS.to_vec();
        markers.sort();
        assert_eq!(lit, markers);
    }
}
