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
const GLYPH_3: Glyph6by3 = Glyph6by3 {
    data: [0b00100010, 0b00101001, 0b00110110],
};
const GLYPH_4: Glyph6by3 = Glyph6by3 {
    data: [0b00111100, 0b000001000, 0b00111111],
};
const GLYPH_5: Glyph6by3 = Glyph6by3 {
    data: [0b00111111, 0b00001001, 0b00000110],
};
const GLYPH_6: Glyph6by3 = Glyph6by3 {
    data: [0b00011110, 0b00101001, 0b00000110],
};
const GLYPH_7: Glyph6by3 = Glyph6by3 {
    data: [0b00100011, 0b00101100, 0b00110000],
};
const GLYPH_8: Glyph6by3 = Glyph6by3 {
    data: [0b00010110, 0b00101001, 0b00101001],
};
const GLYPH_9: Glyph6by3 = Glyph6by3 {
    data: [0b00011000, 0b00100101, 0b00011110],
};
pub fn get_glyph(chr: char) -> impl Glyph {
    match chr {
        '0' => &GLYPH_0,
        '1' => &GLYPH_1,
        '2' => &GLYPH_2,
        '3' => &GLYPH_3,
        '4' => &GLYPH_4, // Placeholder for '4', replace with actual glyph
        '5' => &GLYPH_5, // Placeholder for '5', replace with actual glyph
        '6' => &GLYPH_6, // Placeholder for '6', replace with actual glyph
        '7' => &GLYPH_7, // Placeholder for '7', replace with actual glyph
        '8' => &GLYPH_8, // Placeholder for '8', replace with actual glyph
        '9' => &GLYPH_9, // Placeholder for '9', replace with actual glyph
        _ => panic!("Glyph not found"),
    }
}
