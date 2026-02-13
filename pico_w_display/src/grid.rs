use crate::fonts;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio_programs::ws2812::PioWs2812;
use smart_leds::RGB8;

#[allow(dead_code)] // We only use one of these right now
pub enum GridOrigin {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}
#[repr(align(4))]
pub struct Grid<'a, const WIDTH: usize, const SIZE: usize> {
    data: [RGB8; SIZE],
    orientation: GridOrigin,
    foreground: RGB8,
    background: RGB8,
    pio: PioWs2812<'a, PIO0, 0, SIZE>,
}

impl<'d, const WIDTH: usize, const SIZE: usize> Grid<'d, WIDTH, SIZE> {
    pub fn new(pio: PioWs2812<'d, PIO0, 0, SIZE>, orientation: GridOrigin) -> Self {
        Self {
            orientation,
            foreground: RGB8::new(255, 255, 255),
            background: RGB8::new(0, 0, 0),
            pio,
            data: [RGB8::default(); SIZE],
        }
    }

    fn index(&self, x: usize, y: usize) -> usize {
        let height: usize = SIZE / WIDTH;
        // Check if the x and y coordinates are within bounds
        // The bounds check depends on the orientation of the grid
        match self.orientation {
            GridOrigin::BottomLeft | GridOrigin::TopRight => {
                if (x >= WIDTH) || (y >= height) {
                    panic!(
                        "Index out of bounds ({}, {}) - ({}, {})",
                        WIDTH, height, x, y
                    )
                }
            }
            GridOrigin::TopLeft | GridOrigin::BottomRight => {
                if (x >= height) || (y >= WIDTH) {
                    panic!("Index out of bounds ({}, {})", x, y)
                }
            }
        };
        // Calculate the index based on the orientation
        match self.orientation {
            GridOrigin::BottomLeft => y * WIDTH + x,
            GridOrigin::TopLeft => x * WIDTH + (WIDTH - 1 - y),
            GridOrigin::TopRight => (height - 1 - y) * WIDTH + (WIDTH - 1 - x),
            GridOrigin::BottomRight => (height - 1 - x) * WIDTH + y,
        }
    }

    pub fn set(&mut self, x: usize, y: usize, color: RGB8) {
        self.data[self.index(x, y)] = color;
    }

    pub fn on(&mut self, x: usize, y: usize) {
        self.set(x, y, self.foreground);
    }

    pub fn off(&mut self, x: usize, y: usize) {
        self.set(x, y, self.background);
    }

    pub fn clear(&mut self) {
        for i in 0..SIZE {
            self.data[i] = self.background;
        }
    }

    fn clear_rect(&mut self, rect: &Rect) {
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                self.set(x, y, self.background);
            }
        }
    }
    pub async fn update(&mut self) {
        self.pio.write(&self.data).await;
    }

    pub fn set_foreground(&mut self, color: RGB8) {
        self.foreground = color;
    }
    pub fn set_background(&mut self, color: RGB8) {
        self.background = color;
    }

    pub fn blit_glyph(&mut self, x: usize, y: usize, glyph: impl fonts::Glyph) {
        let rect = Rect {
            x,
            y,
            width: glyph.width() as usize,
            height: glyph.height() as usize,
        };
        self.clear_rect(&rect);
        for col in 0..glyph.width() {
            let col_data = glyph.col(col);
            for row in 0..glyph.height() {
                let bit: u32 = 1 << row;
                if (col_data & bit) == bit {
                    self.set(x + col as usize, y + row as usize, self.foreground);
                }
            }
        }
    }

    // pub fn get(&self, x: usize, y: usize) -> RGB8 {
    //     self.data[self.index(x, y)]
    // }
}
