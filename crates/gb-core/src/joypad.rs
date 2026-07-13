//! P1/JOYP joypad register.

pub const INT_JOYPAD: u8 = 1 << 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

impl Button {
    fn mask(self) -> u8 {
        match self {
            Button::Right => 0x01,
            Button::Left => 0x02,
            Button::Up => 0x04,
            Button::Down => 0x08,
            Button::A => 0x10,
            Button::B => 0x20,
            Button::Select => 0x40,
            Button::Start => 0x80,
        }
    }
}

pub struct Joypad {
    /// Bit set = pressed. Low nibble directions, high nibble buttons.
    state: u8,
    select: u8,
}

impl Joypad {
    pub fn new() -> Self {
        Joypad { state: 0, select: 0x30 }
    }

    pub fn set_button(&mut self, b: Button, pressed: bool, iflags: &mut u8) {
        let was = self.state;
        if pressed {
            self.state |= b.mask();
            if was & b.mask() == 0 {
                *iflags |= INT_JOYPAD;
            }
        } else {
            self.state &= !b.mask();
        }
    }

    pub fn read(&self) -> u8 {
        let mut nibble = 0x0F;
        if self.select & 0x10 == 0 {
            nibble &= !(self.state & 0x0F);
        }
        if self.select & 0x20 == 0 {
            nibble &= !(self.state >> 4);
        }
        0xC0 | self.select | nibble
    }

    pub fn write(&mut self, v: u8) {
        self.select = v & 0x30;
    }
}
