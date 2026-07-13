//! Pixel processing unit: scanline renderer with DMG and CGB modes.

pub const INT_VBLANK: u8 = 1 << 0;
pub const INT_STAT: u8 = 1 << 1;

pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;

const MODE_HBLANK: u8 = 0;
const MODE_VBLANK: u8 = 1;
const MODE_OAM: u8 = 2;
const MODE_DRAW: u8 = 3;

const OAM_DOTS: u32 = 80;
const DRAW_DOTS: u32 = 172;
const LINE_DOTS: u32 = 456;

/// DMG shades to RGB ("classic green" palette).
const DMG_COLORS: [[u8; 3]; 4] =
    [[0xE0, 0xF8, 0xD0], [0x88, 0xC0, 0x70], [0x34, 0x68, 0x56], [0x08, 0x18, 0x20]];

pub struct Ppu {
    pub cgb: bool,
    vram: [u8; 0x4000],
    vram_bank: u8,
    pub oam: [u8; 0xA0],

    lcdc: u8,
    stat: u8,
    scy: u8,
    scx: u8,
    ly: u8,
    lyc: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    wy: u8,
    wx: u8,

    // CGB palette RAM.
    bcps: u8,
    ocps: u8,
    bg_pal: [u8; 64],
    obj_pal: [u8; 64],
    /// CGB OPRI: FF6C bit0 set = DMG-style (x-coordinate) sprite priority.
    opri: u8,

    dot: u32,
    mode: u8,
    window_line: u8,
    stat_line: bool,

    pub framebuffer: Box<[u8]>,
    pub frame_done: bool,
}

impl Ppu {
    pub fn new(cgb: bool) -> Self {
        Ppu {
            cgb,
            vram: [0; 0x4000],
            vram_bank: 0,
            oam: [0; 0xA0],
            lcdc: 0x91,
            stat: 0x85,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0,
            obp1: 0,
            wy: 0,
            wx: 0,
            bcps: 0,
            ocps: 0,
            bg_pal: [0xFF; 64],
            obj_pal: [0xFF; 64],
            opri: 0,
            dot: 0,
            mode: MODE_OAM,
            window_line: 0,
            stat_line: false,
            framebuffer: vec![0xFF; SCREEN_W * SCREEN_H * 4].into_boxed_slice(),
            frame_done: false,
        }
    }

    fn lcd_on(&self) -> bool {
        self.lcdc & 0x80 != 0
    }

    pub fn in_hblank(&self) -> bool {
        self.mode == MODE_HBLANK
    }

    /// Advance by video dots.
    pub fn tick(&mut self, dots: u32, iflags: &mut u8) {
        if !self.lcd_on() {
            return;
        }
        for _ in 0..dots {
            self.dot += 1;
            if self.ly < 144 {
                match self.dot {
                    d if d == OAM_DOTS => self.set_mode(MODE_DRAW),
                    d if d == OAM_DOTS + DRAW_DOTS => {
                        self.render_line();
                        self.set_mode(MODE_HBLANK);
                    }
                    d if d == LINE_DOTS => {
                        self.dot = 0;
                        self.ly += 1;
                        if self.ly == 144 {
                            self.set_mode(MODE_VBLANK);
                            *iflags |= INT_VBLANK;
                            self.frame_done = true;
                        } else {
                            self.set_mode(MODE_OAM);
                        }
                    }
                    _ => {}
                }
            } else if self.dot == LINE_DOTS {
                self.dot = 0;
                self.ly += 1;
                if self.ly == 154 {
                    self.ly = 0;
                    self.window_line = 0;
                    self.set_mode(MODE_OAM);
                }
            }
            self.update_stat_line(iflags);
        }
    }

    fn set_mode(&mut self, mode: u8) {
        self.mode = mode;
        self.stat = (self.stat & !0x03) | mode;
    }

    fn update_stat_line(&mut self, iflags: &mut u8) {
        let coincidence = self.ly == self.lyc;
        self.stat = if coincidence { self.stat | 0x04 } else { self.stat & !0x04 };
        let line = (coincidence && self.stat & 0x40 != 0)
            || (self.mode == MODE_HBLANK && self.stat & 0x08 != 0)
            || (self.mode == MODE_VBLANK && self.stat & 0x10 != 0)
            || (self.mode == MODE_OAM && self.stat & 0x20 != 0);
        if line && !self.stat_line {
            *iflags |= INT_STAT;
        }
        self.stat_line = line;
    }

    fn vram_at(&self, bank: usize, addr: u16) -> u8 {
        self.vram[bank * 0x2000 + (addr as usize & 0x1FFF)]
    }

    fn put_pixel(&mut self, x: usize, rgb: [u8; 3]) {
        let i = (self.ly as usize * SCREEN_W + x) * 4;
        self.framebuffer[i] = rgb[0];
        self.framebuffer[i + 1] = rgb[1];
        self.framebuffer[i + 2] = rgb[2];
        self.framebuffer[i + 3] = 0xFF;
    }

    fn cgb_color(pal: &[u8; 64], palette: usize, color: usize) -> [u8; 3] {
        let i = palette * 8 + color * 2;
        let raw = pal[i] as u16 | ((pal[i + 1] as u16) << 8);
        let r = (raw & 0x1F) as u8;
        let g = ((raw >> 5) & 0x1F) as u8;
        let b = ((raw >> 10) & 0x1F) as u8;
        [(r << 3) | (r >> 2), (g << 3) | (g >> 2), (b << 3) | (b >> 2)]
    }

    fn render_line(&mut self) {
        let ly = self.ly as usize;
        if ly >= SCREEN_H {
            return;
        }

        // Per-pixel BG color index (0-3) and CGB BG-over-OBJ priority flag.
        let mut bg_index = [0u8; SCREEN_W];
        let mut bg_priority = [false; SCREEN_W];

        let bg_enabled = self.lcdc & 0x01 != 0 || self.cgb;
        if bg_enabled {
            let window_visible = self.lcdc & 0x20 != 0 && self.wy <= self.ly && self.wx < 167;
            let mut window_used = false;
            for x in 0..SCREEN_W {
                let in_window = window_visible && x + 7 >= self.wx as usize;
                let (tx, ty, map_base) = if in_window {
                    window_used = true;
                    let wx = x + 7 - self.wx as usize;
                    let wy = self.window_line as usize;
                    (wx, wy, if self.lcdc & 0x40 != 0 { 0x9C00 } else { 0x9800 })
                } else {
                    let bx = (x + self.scx as usize) & 0xFF;
                    let by = (ly + self.scy as usize) & 0xFF;
                    (bx, by, if self.lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 })
                };
                let map_addr = map_base + (ty / 8) * 32 + tx / 8;
                let tile_id = self.vram_at(0, map_addr as u16);
                let attrs = if self.cgb { self.vram_at(1, map_addr as u16) } else { 0 };

                let tile_bank = if attrs & 0x08 != 0 { 1 } else { 0 };
                let mut row = ty % 8;
                if attrs & 0x40 != 0 {
                    row = 7 - row;
                }
                let tile_addr = if self.lcdc & 0x10 != 0 {
                    0x8000 + tile_id as usize * 16 + row * 2
                } else {
                    (0x9000i32 + (tile_id as i8 as i32) * 16 + row as i32 * 2) as usize
                };
                let lo = self.vram_at(tile_bank, tile_addr as u16);
                let hi = self.vram_at(tile_bank, (tile_addr + 1) as u16);
                let mut bit = 7 - (tx % 8);
                if attrs & 0x20 != 0 {
                    bit = 7 - bit;
                }
                let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                bg_index[x] = color;
                bg_priority[x] = attrs & 0x80 != 0;

                let rgb = if self.cgb {
                    Self::cgb_color(&self.bg_pal, (attrs & 0x07) as usize, color as usize)
                } else {
                    DMG_COLORS[((self.bgp >> (color * 2)) & 0x03) as usize]
                };
                self.put_pixel(x, rgb);
            }
            if window_used {
                self.window_line += 1;
            }
        } else {
            for x in 0..SCREEN_W {
                self.put_pixel(x, DMG_COLORS[0]);
            }
        }

        if self.lcdc & 0x02 == 0 {
            return;
        }
        let tall = self.lcdc & 0x04 != 0;
        let height = if tall { 16 } else { 8 };

        // OAM scan: first 10 sprites on this line, in OAM order.
        let mut line_sprites: Vec<(usize, [u8; 4])> = Vec::with_capacity(10);
        for i in 0..40 {
            let e: [u8; 4] = self.oam[i * 4..i * 4 + 4].try_into().unwrap();
            let sy = e[0] as i32 - 16;
            if (ly as i32) >= sy && (ly as i32) < sy + height {
                line_sprites.push((i, e));
                if line_sprites.len() == 10 {
                    break;
                }
            }
        }
        // Draw priority: DMG (and CGB with OPRI set) = lower x first wins,
        // ties by OAM order; CGB = OAM order. Draw lowest priority first.
        let dmg_priority = !self.cgb || self.opri & 0x01 != 0;
        if dmg_priority {
            line_sprites.sort_by_key(|(i, e)| (e[1], *i));
        }
        for &(_, e) in line_sprites.iter().rev() {
            let sy = e[0] as i32 - 16;
            let sx = e[1] as i32 - 8;
            let mut tile = e[2];
            if tall {
                tile &= 0xFE;
            }
            let attrs = e[3];
            let mut row = (ly as i32 - sy) as usize;
            if attrs & 0x40 != 0 {
                row = height as usize - 1 - row;
            }
            let bank = if self.cgb && attrs & 0x08 != 0 { 1 } else { 0 };
            let tile_addr = 0x8000 + tile as usize * 16 + row * 2;
            let lo = self.vram_at(bank, tile_addr as u16);
            let hi = self.vram_at(bank, (tile_addr + 1) as u16);
            for px in 0..8i32 {
                let x = sx + px;
                if !(0..SCREEN_W as i32).contains(&x) {
                    continue;
                }
                let x = x as usize;
                let bit = if attrs & 0x20 != 0 { px } else { 7 - px } as u8;
                let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                if color == 0 {
                    continue;
                }
                // BG/window priority over sprite.
                let bg_color = bg_index[x];
                if bg_color != 0 {
                    let obj_behind = attrs & 0x80 != 0;
                    if self.cgb {
                        let master = self.lcdc & 0x01 != 0;
                        if master && (obj_behind || bg_priority[x]) {
                            continue;
                        }
                    } else if obj_behind {
                        continue;
                    }
                }
                let rgb = if self.cgb {
                    Self::cgb_color(&self.obj_pal, (attrs & 0x07) as usize, color as usize)
                } else {
                    let pal = if attrs & 0x10 != 0 { self.obp1 } else { self.obp0 };
                    DMG_COLORS[((pal >> (color * 2)) & 0x03) as usize]
                };
                self.put_pixel(x, rgb);
            }
        }
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram_at(self.vram_bank as usize, addr)
    }

    pub fn write_vram(&mut self, addr: u16, v: u8) {
        self.vram[self.vram_bank as usize * 0x2000 + (addr as usize & 0x1FFF)] = v;
    }

    /// Direct VRAM write used by CGB HDMA (honours the current VBK bank).
    pub fn hdma_write(&mut self, addr: u16, v: u8) {
        self.write_vram(addr, v);
    }

    pub fn read_reg(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => self.stat | 0x80,
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly,
            0xFF45 => self.lyc,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            0xFF4F if self.cgb => self.vram_bank | 0xFE,
            0xFF68 if self.cgb => self.bcps | 0x40,
            0xFF69 if self.cgb => self.bg_pal[(self.bcps & 0x3F) as usize],
            0xFF6A if self.cgb => self.ocps | 0x40,
            0xFF6B if self.cgb => self.obj_pal[(self.ocps & 0x3F) as usize],
            0xFF6C if self.cgb => self.opri | 0xFE,
            _ => 0xFF,
        }
    }

    pub fn write_reg(&mut self, addr: u16, v: u8, iflags: &mut u8) {
        match addr {
            0xFF40 => {
                let was_on = self.lcd_on();
                self.lcdc = v;
                if was_on && !self.lcd_on() {
                    self.ly = 0;
                    self.dot = 0;
                    self.window_line = 0;
                    self.set_mode(MODE_HBLANK);
                    self.stat_line = false;
                } else if !was_on && self.lcd_on() {
                    self.set_mode(MODE_OAM);
                    self.update_stat_line(iflags);
                }
            }
            0xFF41 => {
                self.stat = (self.stat & 0x07) | (v & 0x78);
                self.update_stat_line(iflags);
            }
            0xFF42 => self.scy = v,
            0xFF43 => self.scx = v,
            0xFF45 => {
                self.lyc = v;
                self.update_stat_line(iflags);
            }
            0xFF47 => self.bgp = v,
            0xFF48 => self.obp0 = v,
            0xFF49 => self.obp1 = v,
            0xFF4A => self.wy = v,
            0xFF4B => self.wx = v,
            0xFF4F if self.cgb => self.vram_bank = v & 0x01,
            0xFF68 if self.cgb => self.bcps = v & 0xBF,
            0xFF69 if self.cgb => {
                self.bg_pal[(self.bcps & 0x3F) as usize] = v;
                if self.bcps & 0x80 != 0 {
                    self.bcps = 0x80 | ((self.bcps + 1) & 0x3F);
                }
            }
            0xFF6A if self.cgb => self.ocps = v & 0xBF,
            0xFF6B if self.cgb => {
                self.obj_pal[(self.ocps & 0x3F) as usize] = v;
                if self.ocps & 0x80 != 0 {
                    self.ocps = 0x80 | ((self.ocps + 1) & 0x3F);
                }
            }
            0xFF6C if self.cgb => self.opri = v & 0x01,
            _ => {}
        }
    }
}
