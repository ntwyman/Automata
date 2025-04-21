use core::char;

pub trait Glyph {
    fn width(&self) -> u8;
    fn height(&self) -> u8;
    fn col(&self, col: u8) -> u32;
}

struct Glyph6by3 {
    data: [u8; 3],
}

impl Glyph for &Glyph6by3 {
    fn width(&self) -> u8 {
        3
    }

    fn height(&self) -> u8 {
        6
    }

    fn col(&self, col: u8) -> u32 {
        let col = col as usize;
        if col < 3 { self.data[col] as u32 } else { 0 }
    }
}
const GLYPH_0: Glyph6by3 = Glyph6by3 {
    data: [0b00011110, 0b00100001, 0b00011110],
};
const GLYPH_1: Glyph6by3 = Glyph6by3 {
    data: [0b00010001, 0b00111111, 0b00000001],
};
const GLYPH_2: Glyph6by3 = Glyph6by3 {
    data: [0b00010011, 0b00100101, 0b00011001],
};
pub fn get_glyph(chr: char) -> impl Glyph {
    match chr {
        '0' => &GLYPH_0,
        '1' => &GLYPH_1,
        '2' => &GLYPH_2,
        _ => panic!("Glyph not found"),
    }
}
