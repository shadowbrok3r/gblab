//! PPU: scanline renderer for modes 0-5 with sprites, windows, and blending.

pub const SCREEN_W: usize = 240;
pub const SCREEN_H: usize = 160;

const CYCLES_PER_LINE: u64 = 1232; // 308 dots x 4 cycles
const HBLANK_AT: u64 = 1006;
const LINES: u32 = 228;

/// Transparent marker in line buffers; opaque colors are 15-bit.
const TRANS: u16 = 0x8000;
const BACKDROP: usize = 5;

pub struct Ppu {
    dispcnt: u16,
    dispstat_w: u16, // writable bits: irq enables + VCOUNT setting
    bgcnt: [u16; 4],
    bghofs: [u16; 4],
    bgvofs: [u16; 4],
    // Affine parameters and reference points, index 0 = BG2, 1 = BG3.
    bgpa: [i16; 2],
    bgpb: [i16; 2],
    bgpc: [i16; 2],
    bgpd: [i16; 2],
    bgx_latch: [i32; 2],
    bgy_latch: [i32; 2],
    bgx: [i32; 2], // internal reference, advanced by PB/PD per line
    bgy: [i32; 2],
    mos_ref: [(i32, i32); 2], // affine reference held across a mosaic block
    winh: [u16; 2],
    winv: [u16; 2],
    winin: u16,
    winout: u16,
    mosaic: u16,
    bldcnt: u16,
    bldalpha: u16,
    bldy: u16,
    line: u32,
    line_cycle: u64,
    pub frame_done: bool,
    pub framebuffer: Vec<u8>, // RGBA8
    pub palette: Vec<u8>,
    pub vram: Vec<u8>,
    pub oam: Vec<u8>,
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            dispcnt: 0x0080,
            dispstat_w: 0,
            bgcnt: [0; 4],
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bgpa: [0x100; 2],
            bgpb: [0; 2],
            bgpc: [0; 2],
            bgpd: [0x100; 2],
            bgx_latch: [0; 2],
            bgy_latch: [0; 2],
            bgx: [0; 2],
            bgy: [0; 2],
            mos_ref: [(0, 0); 2],
            winh: [0; 2],
            winv: [0; 2],
            winin: 0,
            winout: 0,
            mosaic: 0,
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            line: 0,
            line_cycle: 0,
            frame_done: false,
            framebuffer: vec![0; SCREEN_W * SCREEN_H * 4],
            palette: vec![0; 1024],
            vram: vec![0; 96 * 1024],
            oam: vec![0; 1024],
        }
    }

    pub fn io_read16(&mut self, addr: u32) -> u16 {
        match addr {
            0x00 => self.dispcnt,
            0x04 => self.dispstat(),
            0x06 => self.line as u16,
            0x08..=0x0E => self.bgcnt[((addr - 8) / 2) as usize],
            0x48 => self.winin,
            0x4A => self.winout,
            0x50 => self.bldcnt,
            0x52 => self.bldalpha,
            _ => 0,
        }
    }

    pub fn io_write16(&mut self, addr: u32, v: u16) {
        match addr {
            0x00 => self.dispcnt = v,
            0x04 => self.dispstat_w = v & 0xFF38,
            0x08..=0x0E => self.bgcnt[((addr - 8) / 2) as usize] = v,
            0x10..=0x1E => {
                let n = ((addr - 0x10) / 4) as usize;
                if addr & 2 == 0 {
                    self.bghofs[n] = v & 0x1FF;
                } else {
                    self.bgvofs[n] = v & 0x1FF;
                }
            }
            0x20..=0x3E => self.write_affine(addr, v),
            0x40 => self.winh[0] = v,
            0x42 => self.winh[1] = v,
            0x44 => self.winv[0] = v,
            0x46 => self.winv[1] = v,
            0x48 => self.winin = v & 0x3F3F,
            0x4A => self.winout = v & 0x3F3F,
            0x4C => self.mosaic = v,
            0x50 => self.bldcnt = v & 0x3FFF,
            0x52 => self.bldalpha = v & 0x1F1F,
            0x54 => self.bldy = v & 0x1F,
            _ => {}
        }
    }

    /// BG2/BG3 PA-PD and X/Y reference points (0x20-0x2E, 0x30-0x3E).
    fn write_affine(&mut self, addr: u32, v: u16) {
        let n = ((addr >> 4) - 2) as usize;
        match addr & 0xE {
            0x0 => self.bgpa[n] = v as i16,
            0x2 => self.bgpb[n] = v as i16,
            0x4 => self.bgpc[n] = v as i16,
            0x6 => self.bgpd[n] = v as i16,
            _ => {
                let r = if addr & 0xC == 0x8 { &mut self.bgx_latch[n] } else { &mut self.bgy_latch[n] };
                if addr & 2 == 0 {
                    *r = (*r & !0xFFFF) | v as i32;
                } else {
                    *r = (*r & 0xFFFF) | ((v as i32 & 0x0FFF) << 16);
                }
                *r = (*r << 4) >> 4; // sign-extend 28 bits
                // Writes take effect immediately on the internal register.
                if addr & 0xC == 0x8 {
                    self.bgx[n] = self.bgx_latch[n];
                } else {
                    self.bgy[n] = self.bgy_latch[n];
                }
            }
        }
    }

    fn dispstat(&self) -> u16 {
        let in_vblank = (160..227).contains(&self.line);
        let in_hblank = self.line_cycle >= HBLANK_AT;
        let vmatch = self.line as u16 == self.dispstat_w >> 8;
        self.dispstat_w | in_vblank as u16 | (in_hblank as u16) << 1 | (vmatch as u16) << 2
    }

    pub fn tick(&mut self, cycles: u64, if_reg: &mut u16, dma: &mut crate::dma::Dma) {
        let mut left = cycles;
        while left > 0 {
            let next = if self.line_cycle < HBLANK_AT { HBLANK_AT } else { CYCLES_PER_LINE };
            let step = left.min(next - self.line_cycle);
            self.line_cycle += step;
            left -= step;
            if self.line_cycle == HBLANK_AT {
                self.enter_hblank(if_reg, dma);
            }
            if self.line_cycle == CYCLES_PER_LINE {
                self.next_line(if_reg, dma);
            }
        }
    }

    fn enter_hblank(&mut self, if_reg: &mut u16, dma: &mut crate::dma::Dma) {
        if self.line < 160 {
            self.render_scanline(self.line as usize);
            for n in 0..2 {
                self.bgx[n] += self.bgpb[n] as i32;
                self.bgy[n] += self.bgpd[n] as i32;
            }
            dma.request(crate::dma::DmaTiming::HBlank);
        }
        if self.dispstat_w & 0x10 != 0 {
            *if_reg |= 1 << 1;
        }
    }

    fn next_line(&mut self, if_reg: &mut u16, dma: &mut crate::dma::Dma) {
        self.line_cycle = 0;
        self.line += 1;
        if self.line == LINES {
            self.line = 0;
            self.bgx = self.bgx_latch;
            self.bgy = self.bgy_latch;
        }
        if self.line == 160 {
            self.frame_done = true;
            dma.request(crate::dma::DmaTiming::VBlank);
            if self.dispstat_w & 0x08 != 0 {
                *if_reg |= 1;
            }
        }
        if self.line as u16 == self.dispstat_w >> 8 && self.dispstat_w & 0x20 != 0 {
            *if_reg |= 1 << 2;
        }
    }

    pub(crate) fn render_scanline(&mut self, y: usize) {
        let row = &mut self.framebuffer[y * SCREEN_W * 4..(y + 1) * SCREEN_W * 4];
        if self.dispcnt & 0x80 != 0 {
            row.fill(0xFF); // forced blank shows white
            return;
        }

        let mut bg = [[TRANS; SCREEN_W]; 4];
        let mode = self.dispcnt & 7;
        for n in 0..4 {
            if self.dispcnt & (0x100 << n) == 0 {
                continue;
            }
            match (mode, n) {
                (0, _) | (1, 0) | (1, 1) => self.render_text_bg(n, y, &mut bg[n]),
                (1, 2) | (2, 2) | (2, 3) => self.render_affine_bg(n, y, &mut bg[n]),
                (3..=5, 2) => self.render_bitmap_bg(mode, y, &mut bg[2]),
                _ => {}
            }
        }

        let mut obj_color = [TRANS; SCREEN_W];
        let mut obj_prio = [4u8; SCREEN_W];
        let mut obj_semi = [false; SCREEN_W];
        let mut obj_win = [false; SCREEN_W];
        if self.dispcnt & 0x1000 != 0 {
            self.render_objects(y, &mut obj_color, &mut obj_prio, &mut obj_semi, &mut obj_win);
        }

        let backdrop = u16::from_le_bytes([self.palette[0], self.palette[1]]) & 0x7FFF;
        let eva = 16.min(self.bldalpha & 0x1F) as u32;
        let evb = 16.min((self.bldalpha >> 8) & 0x1F) as u32;
        let evy = 16.min(self.bldy) as u32;
        let blend_mode = (self.bldcnt >> 6) & 3;

        for x in 0..SCREEN_W {
            let ctl = self.window_control(x, y, obj_win[x]);
            // Top two visible layers by priority; OBJ beats same-priority BGs.
            let mut hits = 0;
            let mut c1 = backdrop;
            let mut l1 = BACKDROP;
            let mut semi1 = false;
            let mut c2 = backdrop;
            let mut l2 = BACKDROP;
            'stack: for p in 0..4u8 {
                if ctl & 0x10 != 0 && obj_prio[x] == p && obj_color[x] != TRANS {
                    if hits == 0 {
                        c1 = obj_color[x];
                        l1 = 4;
                        semi1 = obj_semi[x];
                        hits = 1;
                    } else {
                        c2 = obj_color[x];
                        l2 = 4;
                        break 'stack;
                    }
                }
                for n in 0..4 {
                    if self.dispcnt & (0x100 << n) != 0
                        && ctl & (1 << n) != 0
                        && self.bgcnt[n] & 3 == p as u16
                        && bg[n][x] != TRANS
                    {
                        if hits == 0 {
                            c1 = bg[n][x];
                            l1 = n;
                            hits = 1;
                        } else {
                            c2 = bg[n][x];
                            l2 = n;
                            break 'stack;
                        }
                    }
                }
            }

            let blend_ok = ctl & 0x20 != 0;
            let t1 = self.bldcnt & (1 << l1) != 0;
            let t2 = self.bldcnt & (0x100 << l2) != 0;
            let mut out = c1;
            if semi1 && t2 && blend_ok {
                out = alpha_blend(c1, c2, eva, evb);
            } else if blend_ok && t1 {
                match blend_mode {
                    1 if t2 => out = alpha_blend(c1, c2, eva, evb),
                    2 => out = brightness(c1, evy, true),
                    3 => out = brightness(c1, evy, false),
                    _ => {}
                }
            }
            put_rgb555(&mut self.framebuffer[(y * SCREEN_W + x) * 4..], out);
        }
    }

    /// Window layer-control bits for a pixel: 0-3 BGs, 4 OBJ, 5 blend.
    fn window_control(&self, x: usize, y: usize, obj_win: bool) -> u16 {
        if self.dispcnt & 0xE000 == 0 {
            return 0x3F;
        }
        for w in 0..2 {
            if self.dispcnt & (0x2000 << w) != 0
                && in_win_range(self.winh[w], x, 240)
                && in_win_range(self.winv[w], y, 160)
            {
                return (self.winin >> (8 * w)) & 0x3F;
            }
        }
        if self.dispcnt & 0x8000 != 0 && obj_win {
            return (self.winout >> 8) & 0x3F;
        }
        self.winout & 0x3F
    }

    fn render_text_bg(&mut self, n: usize, y: usize, out: &mut [u16; SCREEN_W]) {
        let cnt = self.bgcnt[n];
        let char_base = ((cnt >> 2) & 3) as usize * 0x4000;
        let screen_base = ((cnt >> 8) & 0x1F) as usize * 0x800;
        let bpp8 = cnt & 0x80 != 0;
        let wide = cnt & 0x4000 != 0;
        let tall = cnt & 0x8000 != 0;
        let (mh, mv) = self.bg_mosaic(cnt);
        let sy = (y - y % mv + self.bgvofs[n] as usize) & if tall { 511 } else { 255 };
        // Bottom half of a tall map: +1 block at 256 wide, +2 at 512 wide.
        let row_block = if sy >= 256 { if wide { 2 } else { 1 } } else { 0 };
        let ty = (sy % 256) / 8;
        let fy = sy % 8;
        for x in 0..SCREEN_W {
            let sx = (x + self.bghofs[n] as usize) & if wide { 511 } else { 255 };
            let block = row_block + (sx >= 256) as usize;
            let ei = screen_base + block * 0x800 + (ty * 32 + (sx % 256) / 8) * 2;
            let entry = u16::from_le_bytes([self.vram[ei], self.vram[ei + 1]]);
            let tile = (entry & 0x3FF) as usize;
            let px = if entry & 0x400 != 0 { 7 - sx % 8 } else { sx % 8 };
            let py = if entry & 0x800 != 0 { 7 - fy } else { fy };
            out[x] = if bpp8 {
                let a = char_base + tile * 64 + py * 8 + px;
                let v = if a >= 0x10000 { 0 } else { self.vram[a] as usize };
                if v == 0 { TRANS } else { self.bg_color(v) }
            } else {
                let a = char_base + tile * 32 + py * 4 + px / 2;
                let b = if a >= 0x10000 { 0 } else { self.vram[a] };
                let v = if px & 1 != 0 { b >> 4 } else { b & 0xF } as usize;
                if v == 0 { TRANS } else { self.bg_color(((entry >> 12) & 0xF) as usize * 16 + v) }
            };
        }
        apply_h_mosaic(out, mh);
    }

    fn render_affine_bg(&mut self, n: usize, y: usize, out: &mut [u16; SCREEN_W]) {
        let cnt = self.bgcnt[n];
        let i = n - 2;
        let char_base = ((cnt >> 2) & 3) as usize * 0x4000;
        let screen_base = ((cnt >> 8) & 0x1F) as usize * 0x800;
        let wrap = cnt & 0x2000 != 0;
        let size = 128usize << (cnt >> 14);
        let (mh, mv) = self.bg_mosaic(cnt);
        let (mut cx, mut cy) = self.affine_ref(i, y, mv, cnt & 0x40 != 0);
        for x in 0..SCREEN_W {
            let (px, py) = (cx >> 8, cy >> 8);
            cx += self.bgpa[i] as i32;
            cy += self.bgpc[i] as i32;
            let (px, py) = if wrap {
                (px as usize & (size - 1), py as usize & (size - 1))
            } else if px < 0 || py < 0 || px >= size as i32 || py >= size as i32 {
                out[x] = TRANS;
                continue;
            } else {
                (px as usize, py as usize)
            };
            let tile = self.vram[screen_base + (py / 8) * (size / 8) + px / 8] as usize;
            let a = char_base + tile * 64 + (py % 8) * 8 + px % 8;
            let v = if a >= 0x10000 { 0 } else { self.vram[a] } as usize;
            out[x] = if v == 0 { TRANS } else { self.bg_color(v) };
        }
        apply_h_mosaic(out, mh);
    }

    fn render_bitmap_bg(&mut self, mode: u16, y: usize, out: &mut [u16; SCREEN_W]) {
        let cnt = self.bgcnt[2];
        let page = if self.dispcnt & 0x10 != 0 { 0xA000 } else { 0 };
        let (bw, bh) = if mode == 5 { (160, 128) } else { (240, 160) };
        let (mh, mv) = self.bg_mosaic(cnt);
        let (mut cx, mut cy) = self.affine_ref(0, y, mv, cnt & 0x40 != 0);
        for x in 0..SCREEN_W {
            let (px, py) = (cx >> 8, cy >> 8);
            cx += self.bgpa[0] as i32;
            cy += self.bgpc[0] as i32;
            if px < 0 || py < 0 || px >= bw || py >= bh {
                out[x] = TRANS;
                continue;
            }
            let i = (py * bw + px) as usize;
            out[x] = match mode {
                3 => u16::from_le_bytes([self.vram[i * 2], self.vram[i * 2 + 1]]) & 0x7FFF,
                4 => {
                    let v = self.vram[page + i] as usize;
                    if v == 0 { TRANS } else { self.bg_color(v) }
                }
                _ => {
                    let a = page + i * 2;
                    u16::from_le_bytes([self.vram[a], self.vram[a + 1]]) & 0x7FFF
                }
            };
        }
        apply_h_mosaic(out, mh);
    }

    /// Affine start coordinates, held at the block top when mosaic is on.
    fn affine_ref(&mut self, i: usize, y: usize, mv: usize, mosaic: bool) -> (i32, i32) {
        if mosaic {
            if y % mv == 0 {
                self.mos_ref[i] = (self.bgx[i], self.bgy[i]);
            }
            self.mos_ref[i]
        } else {
            (self.bgx[i], self.bgy[i])
        }
    }

    fn bg_mosaic(&self, cnt: u16) -> (usize, usize) {
        if cnt & 0x40 != 0 {
            ((self.mosaic & 0xF) as usize + 1, ((self.mosaic >> 4) & 0xF) as usize + 1)
        } else {
            (1, 1)
        }
    }

    fn bg_color(&self, index: usize) -> u16 {
        u16::from_le_bytes([self.palette[index * 2], self.palette[index * 2 + 1]]) & 0x7FFF
    }

    fn render_objects(
        &mut self,
        y: usize,
        color: &mut [u16; SCREEN_W],
        prio: &mut [u8; SCREEN_W],
        semi: &mut [bool; SCREEN_W],
        objwin: &mut [bool; SCREEN_W],
    ) {
        let map_1d = self.dispcnt & 0x40 != 0;
        let bitmap_mode = self.dispcnt & 7 >= 3;
        for s in 0..128 {
            let a0 = u16::from_le_bytes([self.oam[s * 8], self.oam[s * 8 + 1]]);
            let a1 = u16::from_le_bytes([self.oam[s * 8 + 2], self.oam[s * 8 + 3]]);
            let a2 = u16::from_le_bytes([self.oam[s * 8 + 4], self.oam[s * 8 + 5]]);
            let affine = a0 & 0x100 != 0;
            if !affine && a0 & 0x200 != 0 {
                continue; // disabled
            }
            let mode = (a0 >> 10) & 3;
            if mode == 3 {
                continue;
            }
            let (w, h) = obj_size((a0 >> 14) as usize, (a1 >> 14) as usize);
            let (bw, bh) = if affine && a0 & 0x200 != 0 { (w * 2, h * 2) } else { (w, h) };
            let row = (y as i32 - (a0 & 0xFF) as i32) & 0xFF;
            if row >= bh {
                continue;
            }
            let x0 = if a1 & 0x1FF >= 256 { (a1 & 0x1FF) as i32 - 512 } else { (a1 & 0x1FF) as i32 };
            let bpp8 = a0 & 0x2000 != 0;
            let tile_base = (a2 & 0x3FF) as usize * 32;
            let stride = if map_1d { w as usize / 8 * if bpp8 { 2 } else { 1 } } else { 32 };
            let pri = ((a2 >> 10) & 3) as u8;
            let palbank = ((a2 >> 12) & 0xF) as usize;
            let params = if affine {
                let g = ((a1 >> 9) & 0x1F) as usize * 32;
                Some([6, 14, 22, 30].map(|o| i16::from_le_bytes([self.oam[g + o], self.oam[g + o + 1]]) as i32))
            } else {
                None
            };
            for i in 0..bw {
                let x = x0 + i;
                if !(0..SCREEN_W as i32).contains(&x) {
                    continue;
                }
                let x = x as usize;
                let (tx, ty) = if let Some([pa, pb, pc, pd]) = params {
                    let (dx, dy) = (i - bw / 2, row - bh / 2);
                    let tx = ((pa * dx + pb * dy) >> 8) + w / 2;
                    let ty = ((pc * dx + pd * dy) >> 8) + h / 2;
                    if tx < 0 || ty < 0 || tx >= w || ty >= h {
                        continue;
                    }
                    (tx as usize, ty as usize)
                } else {
                    let tx = if a1 & 0x1000 != 0 { w - 1 - i } else { i } as usize;
                    let ty = if a1 & 0x2000 != 0 { h - 1 - row } else { row } as usize;
                    (tx, ty)
                };
                let cell = if bpp8 { 64 } else { 32 };
                let off = (tile_base + (ty / 8) * stride * 32 + (tx / 8) * cell) & 0x7FFF;
                if bitmap_mode && off < 0x4000 {
                    continue; // obj tiles below 512 hidden in modes 3-5
                }
                let v = if bpp8 {
                    self.vram[0x10000 + ((off + (ty % 8) * 8 + tx % 8) & 0x7FFF)] as usize
                } else {
                    let b = self.vram[0x10000 + ((off + (ty % 8) * 4 + tx % 8 / 2) & 0x7FFF)];
                    (if tx & 1 != 0 { b >> 4 } else { b & 0xF }) as usize
                };
                if v == 0 {
                    continue;
                }
                if mode == 2 {
                    objwin[x] = true;
                    continue;
                }
                if color[x] != TRANS {
                    continue; // lower OAM index keeps the pixel
                }
                let pi = 0x200 + if bpp8 { v } else { palbank * 16 + v } * 2;
                color[x] = u16::from_le_bytes([self.palette[pi], self.palette[pi + 1]]) & 0x7FFF;
                prio[x] = pri;
                semi[x] = mode == 1;
            }
        }
    }
}

/// GBATEK garbage handling: x2 > max or x2 < x1 extends the window to max.
fn in_win_range(reg: u16, v: usize, max: usize) -> bool {
    let x1 = (reg >> 8) as usize;
    let mut x2 = (reg & 0xFF) as usize;
    if x2 > max || x2 < x1 {
        x2 = max;
    }
    x1 <= v && v < x2
}

fn apply_h_mosaic(buf: &mut [u16; SCREEN_W], mh: usize) {
    if mh > 1 {
        for x in 0..SCREEN_W {
            buf[x] = buf[x - x % mh];
        }
    }
}

fn obj_size(shape: usize, size: usize) -> (i32, i32) {
    const SIZES: [[(i32, i32); 4]; 3] = [
        [(8, 8), (16, 16), (32, 32), (64, 64)],
        [(16, 8), (32, 8), (32, 16), (64, 32)],
        [(8, 16), (8, 32), (16, 32), (32, 64)],
    ];
    SIZES[shape.min(2)][size]
}

fn alpha_blend(c1: u16, c2: u16, eva: u32, evb: u32) -> u16 {
    let mut out = 0;
    for sh in [0, 5, 10] {
        let a = (c1 >> sh) as u32 & 0x1F;
        let b = (c2 >> sh) as u32 & 0x1F;
        out |= (31.min((a * eva + b * evb) / 16) as u16) << sh;
    }
    out
}

fn brightness(c: u16, evy: u32, up: bool) -> u16 {
    let mut out = 0;
    for sh in [0, 5, 10] {
        let v = (c >> sh) as u32 & 0x1F;
        let v = if up { v + (31 - v) * evy / 16 } else { v - v * evy / 16 };
        out |= (v as u16) << sh;
    }
    out
}

fn put_rgb555(px: &mut [u8], c: u16) {
    let r = (c & 0x1F) as u8;
    let g = ((c >> 5) & 0x1F) as u8;
    let b = ((c >> 10) & 0x1F) as u8;
    px[0] = (r << 3) | (r >> 2);
    px[1] = (g << 3) | (g >> 2);
    px[2] = (b << 3) | (b >> 2);
    px[3] = 0xFF;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_mode(mode: u16, extra: u16) -> Ppu {
        let mut p = Ppu::new();
        p.io_write16(0x00, mode | extra);
        p
    }

    fn dma() -> crate::dma::Dma {
        crate::dma::Dma::new()
    }

    fn pal(p: &mut Ppu, i: usize, c: u16) {
        p.palette[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
    }

    fn opal(p: &mut Ppu, i: usize, c: u16) {
        pal(p, 256 + i, c);
    }

    fn px(p: &Ppu, x: usize, y: usize) -> [u8; 3] {
        let i = (y * SCREEN_W + x) * 4;
        [p.framebuffer[i], p.framebuffer[i + 1], p.framebuffer[i + 2]]
    }

    fn rgb(c: u16) -> [u8; 3] {
        let mut b = [0u8; 4];
        put_rgb555(&mut b, c);
        [b[0], b[1], b[2]]
    }

    fn solid_tile4(p: &mut Ppu, addr: usize, v: u8) {
        p.vram[addr..addr + 32].fill(v | v << 4);
    }

    fn map16(p: &mut Ppu, addr: usize, e: u16) {
        p.vram[addr..addr + 2].copy_from_slice(&e.to_le_bytes());
    }

    fn obj(p: &mut Ppu, i: usize, a0: u16, a1: u16, a2: u16) {
        p.oam[i * 8..i * 8 + 2].copy_from_slice(&a0.to_le_bytes());
        p.oam[i * 8 + 2..i * 8 + 4].copy_from_slice(&a1.to_le_bytes());
        p.oam[i * 8 + 4..i * 8 + 6].copy_from_slice(&a2.to_le_bytes());
    }

    fn obj_affine(p: &mut Ppu, g: usize, pa: i16, pb: i16, pc: i16, pd: i16) {
        for (o, v) in [(6, pa), (14, pb), (22, pc), (30, pd)] {
            p.oam[g * 32 + o..g * 32 + o + 2].copy_from_slice(&v.to_le_bytes());
        }
    }

    /// BG0 covering the whole screen with palette entry 1 via solid tile 0.
    fn full_bg0(p: &mut Ppu, prio: u16) {
        p.io_write16(0x08, (8 << 8) | prio);
        solid_tile4(p, 0, 1);
    }

    /// OBJ tile `t` (4bpp) filled with pixel value 1.
    fn solid_obj_tile(p: &mut Ppu, t: usize) {
        let a = 0x10000 + t * 32;
        p.vram[a..a + 32].fill(0x11);
    }

    #[test]
    fn backdrop_fills_screen() {
        let mut p = ppu_mode(0, 0);
        pal(&mut p, 0, 0x1234);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x1234));
        assert_eq!(px(&p, 239, 0), rgb(0x1234));
    }

    #[test]
    fn forced_blank_is_white() {
        let mut p = ppu_mode(0, 0x80);
        pal(&mut p, 0, 0x1234);
        p.render_scanline(0);
        assert_eq!(px(&p, 120, 0), [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn text_4bpp_flips_and_palbank() {
        let mut p = ppu_mode(0, 0x0100);
        p.io_write16(0x08, 8 << 8); // screen base 0x4000, char base 0
        // Tile 1: row0 = 1,2,2,2,2,2,2,3; row7 = 4,0,...
        p.vram[32] = 0x21;
        p.vram[33] = 0x22;
        p.vram[34] = 0x22;
        p.vram[35] = 0x32;
        p.vram[60] = 0x04;
        for (i, c) in [(0, 0x0011), (17, 0x7C00), (19, 0x001F), (20, 0x0F0F), (33, 0x7FFF), (35, 0x4321)] {
            pal(&mut p, i, c);
        }
        map16(&mut p, 0x4000, 0x1001); // tile 1, palbank 1
        map16(&mut p, 0x4002, 0x2401); // tile 1, hflip, palbank 2
        map16(&mut p, 0x4004, 0x1801); // tile 1, vflip, palbank 1
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x7C00)); // bank1 color 1
        assert_eq!(px(&p, 7, 0), rgb(0x001F)); // bank1 color 3
        assert_eq!(px(&p, 8, 0), rgb(0x4321)); // hflip: color 3, bank2
        assert_eq!(px(&p, 15, 0), rgb(0x7FFF)); // hflip: color 1, bank2
        assert_eq!(px(&p, 16, 0), rgb(0x0F0F)); // vflip: row7 color 4
        assert_eq!(px(&p, 17, 0), rgb(0x0011)); // vflip: row7 transparent
    }

    #[test]
    fn text_8bpp() {
        let mut p = ppu_mode(0, 0x0100);
        p.io_write16(0x08, (8 << 8) | 0x80);
        p.vram[64] = 7; // tile 1 pixel (0,0)
        p.vram[64 + 63] = 9; // tile 1 pixel (7,7)
        pal(&mut p, 7, 0x03E0);
        pal(&mut p, 9, 0x001F);
        map16(&mut p, 0x4000, 1);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x03E0));
        p.render_scanline(7);
        assert_eq!(px(&p, 7, 7), rgb(0x001F));
    }

    #[test]
    fn text_scroll_wrap() {
        let mut p = ppu_mode(0, 0x0100);
        p.io_write16(0x08, 8 << 8);
        p.io_write16(0x10, 252); // hofs
        p.io_write16(0x12, 8); // vofs -> tile row 1
        solid_tile4(&mut p, 32, 1);
        solid_tile4(&mut p, 64, 2);
        pal(&mut p, 1, 0x001F);
        pal(&mut p, 2, 0x03E0);
        map16(&mut p, 0x4000 + (32 + 31) * 2, 1); // (31,1) = tile 1
        map16(&mut p, 0x4000 + 32 * 2, 2); // (0,1) = tile 2
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F));
        assert_eq!(px(&p, 3, 0), rgb(0x001F));
        assert_eq!(px(&p, 4, 0), rgb(0x03E0)); // wrapped to sx=0
    }

    #[test]
    fn text_512_wide_screenblock_crossing() {
        let mut p = ppu_mode(0, 0x0100);
        p.io_write16(0x08, (8 << 8) | 0x4000); // 512x256
        p.io_write16(0x10, 248);
        solid_tile4(&mut p, 32, 1);
        solid_tile4(&mut p, 64, 2);
        pal(&mut p, 1, 0x001F);
        pal(&mut p, 2, 0x03E0);
        map16(&mut p, 0x4000 + 31 * 2, 1); // block 0 (31,0)
        map16(&mut p, 0x4800, 2); // block 1 (0,0)
        p.render_scanline(0);
        assert_eq!(px(&p, 7, 0), rgb(0x001F)); // sx=255
        assert_eq!(px(&p, 8, 0), rgb(0x03E0)); // sx=256 -> +1 block
    }

    #[test]
    fn affine_bg_scale() {
        let mut p = ppu_mode(2, 0x0400);
        p.io_write16(0x0C, 1 << 2); // char base 0x4000, screen base 0, 128px
        p.io_write16(0x20, 0x80); // PA = 0.5 -> 2x magnification
        p.vram[0] = 1;
        p.vram[1] = 2;
        p.vram[0x4000 + 64..0x4000 + 128].fill(5);
        p.vram[0x4000 + 128..0x4000 + 192].fill(6);
        pal(&mut p, 5, 0x001F);
        pal(&mut p, 6, 0x03E0);
        p.render_scanline(0);
        assert_eq!(px(&p, 15, 0), rgb(0x001F)); // texel 7 -> tile 1
        assert_eq!(px(&p, 16, 0), rgb(0x03E0)); // texel 8 -> tile 2
    }

    #[test]
    fn affine_bg_wrap_vs_transparent() {
        let mut p = ppu_mode(2, 0x0400);
        p.io_write16(0x0C, (1 << 2) | 0x2000); // wrap on
        p.io_write16(0x28, 127 << 8); // ref x = 127.0
        p.vram[15] = 1; // map (15,0)
        p.vram[0] = 2; // map (0,0)
        p.vram[0x4000 + 64..0x4000 + 128].fill(5);
        p.vram[0x4000 + 128..0x4000 + 192].fill(6);
        pal(&mut p, 0, 0x1111);
        pal(&mut p, 5, 0x001F);
        pal(&mut p, 6, 0x03E0);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F)); // texel 127
        assert_eq!(px(&p, 1, 0), rgb(0x03E0)); // wrapped to texel 0
        p.io_write16(0x0C, 1 << 2); // wrap off
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F));
        assert_eq!(px(&p, 1, 0), rgb(0x1111)); // outside -> backdrop
    }

    #[test]
    fn mode3_direct_color() {
        let mut p = ppu_mode(3, 0x0400);
        let i = (5 * 240 + 10) * 2;
        p.vram[i..i + 2].copy_from_slice(&0x7C1Fu16.to_le_bytes());
        p.bgy[0] = 5 << 8; // internal ref tracks the row during ticking
        p.render_scanline(5);
        assert_eq!(px(&p, 10, 5), rgb(0x7C1F));
        assert_eq!(px(&p, 11, 5), rgb(0));
    }

    #[test]
    fn mode4_page_flip() {
        let mut p = ppu_mode(4, 0x0400);
        pal(&mut p, 0, 0x1111);
        pal(&mut p, 1, 0x001F);
        pal(&mut p, 2, 0x03E0);
        p.vram[5 * 240 + 3] = 1;
        p.vram[0xA000 + 5 * 240 + 3] = 2;
        p.bgy[0] = 5 << 8;
        p.render_scanline(5);
        assert_eq!(px(&p, 3, 5), rgb(0x001F));
        assert_eq!(px(&p, 4, 5), rgb(0x1111)); // index 0 -> backdrop
        p.io_write16(0x00, 4 | 0x0400 | 0x10); // frame 1
        p.render_scanline(5);
        assert_eq!(px(&p, 3, 5), rgb(0x03E0));
    }

    #[test]
    fn mode5_small_bitmap() {
        let mut p = ppu_mode(5, 0x0400);
        pal(&mut p, 0, 0x1111);
        let i = (5 * 160 + 10) * 2;
        p.vram[i..i + 2].copy_from_slice(&0x03FFu16.to_le_bytes());
        p.bgy[0] = 5 << 8;
        p.render_scanline(5);
        assert_eq!(px(&p, 10, 5), rgb(0x03FF));
        assert_eq!(px(&p, 200, 5), rgb(0x1111)); // outside 160x128
        p.bgy[0] = 130 << 8;
        p.render_scanline(130);
        assert_eq!(px(&p, 10, 130), rgb(0x1111));
    }

    #[test]
    fn sprite_priority_vs_bg() {
        let mut p = ppu_mode(0, 0x1100);
        full_bg0(&mut p, 1);
        pal(&mut p, 1, 0x7C00);
        opal(&mut p, 1, 0x001F);
        solid_obj_tile(&mut p, 1);
        obj(&mut p, 0, 0, 0, 1); // 8x8 at (0,0), priority 0
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F)); // obj above
        assert_eq!(px(&p, 8, 0), rgb(0x7C00)); // bg elsewhere
        obj(&mut p, 0, 0, 0, 1 | 2 << 10); // priority 2, below bg
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x7C00));
        obj(&mut p, 0, 0, 0, 1 | 1 << 10); // tie: obj beats same-priority bg
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F));
    }

    #[test]
    fn sprite_oam_index_precedence() {
        // Sprite 0 (prio 1) owns the pixel, so BG prio 0 hides sprite 1 (prio 0).
        let mut p = ppu_mode(0, 0x1100);
        full_bg0(&mut p, 0);
        pal(&mut p, 1, 0x7C00);
        opal(&mut p, 1, 0x001F);
        solid_obj_tile(&mut p, 1);
        obj(&mut p, 0, 0, 0, 1 | 1 << 10);
        obj(&mut p, 1, 0, 0, 1 | 0 << 10);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x7C00));
    }

    #[test]
    fn sprite_hflip() {
        let mut p = ppu_mode(0, 0x1000);
        pal(&mut p, 0, 0);
        opal(&mut p, 1, 0x001F);
        opal(&mut p, 2, 0x03E0);
        p.vram[0x10020] = 0x01; // tile 1 row0: px0=1
        p.vram[0x10023] = 0x20; // px7=2
        obj(&mut p, 0, 0, 0, 1);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F));
        assert_eq!(px(&p, 7, 0), rgb(0x03E0));
        obj(&mut p, 0, 0, 0x1000, 1); // hflip
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x03E0));
        assert_eq!(px(&p, 7, 0), rgb(0x001F));
    }

    #[test]
    fn sprite_affine_identity_double_size() {
        let mut p = ppu_mode(0, 0x1000);
        pal(&mut p, 0, 0x1111);
        opal(&mut p, 1, 0x001F);
        solid_obj_tile(&mut p, 1);
        obj_affine(&mut p, 0, 0x100, 0, 0, 0x100);
        obj(&mut p, 0, 0x300, 0, 1); // affine + double-size, 8x8 -> 16x16 box
        p.render_scanline(0);
        assert_eq!(px(&p, 4, 0), rgb(0x1111)); // above the centered texture
        p.render_scanline(4);
        assert_eq!(px(&p, 3, 4), rgb(0x1111));
        assert_eq!(px(&p, 4, 4), rgb(0x001F)); // texel (0,0)
        assert_eq!(px(&p, 11, 4), rgb(0x001F)); // texel (7,0)
        assert_eq!(px(&p, 12, 4), rgb(0x1111));
    }

    #[test]
    fn sprite_1d_vs_2d_mapping() {
        // 8x16 sprite, row 8: 2D reads tile base+32, 1D reads base+1.
        let mut p = ppu_mode(0, 0x1000);
        pal(&mut p, 0, 0);
        opal(&mut p, 1, 0x001F);
        opal(&mut p, 2, 0x03E0);
        p.vram[0x10000 + 3 * 32..0x10000 + 4 * 32].fill(0x11); // tile 3
        p.vram[0x10000 + 34 * 32..0x10000 + 35 * 32].fill(0x22); // tile 34
        obj(&mut p, 0, 2 << 14, 0, 2);
        p.render_scanline(8);
        assert_eq!(px(&p, 0, 8), rgb(0x03E0)); // 2D
        p.io_write16(0x00, 0x1000 | 0x40);
        p.render_scanline(8);
        assert_eq!(px(&p, 0, 8), rgb(0x001F)); // 1D
    }

    #[test]
    fn sprite_y_and_x_wrap() {
        let mut p = ppu_mode(0, 0x1000);
        pal(&mut p, 0, 0x1111);
        opal(&mut p, 1, 0x001F);
        solid_obj_tile(&mut p, 1);
        obj(&mut p, 0, 252, 508, 1); // y=-4, x=-4
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F)); // texel (4,4)
        assert_eq!(px(&p, 4, 0), rgb(0x1111)); // past sprite right edge
        p.render_scanline(4);
        assert_eq!(px(&p, 0, 4), rgb(0x1111)); // past sprite bottom
    }

    #[test]
    fn bitmap_mode_hides_low_obj_tiles() {
        let mut p = ppu_mode(3, 0x1000);
        pal(&mut p, 0, 0x1111);
        opal(&mut p, 1, 0x001F);
        solid_obj_tile(&mut p, 511);
        solid_obj_tile(&mut p, 512);
        obj(&mut p, 0, 0, 0, 511);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x1111)); // tile < 512 hidden
        obj(&mut p, 0, 0, 0, 512);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F));
    }

    #[test]
    fn window_clips_bg() {
        let mut p = ppu_mode(0, 0x0100 | 0x2000);
        full_bg0(&mut p, 0);
        pal(&mut p, 0, 0x1111);
        pal(&mut p, 1, 0x001F);
        p.io_write16(0x40, (4 << 8) | 8); // win0 x 4..8
        p.io_write16(0x44, 160); // win0 y 0..160
        p.io_write16(0x48, 0x01); // inside: bg0 only
        p.io_write16(0x4A, 0x00); // outside: nothing
        p.render_scanline(0);
        assert_eq!(px(&p, 3, 0), rgb(0x1111));
        assert_eq!(px(&p, 4, 0), rgb(0x001F));
        assert_eq!(px(&p, 7, 0), rgb(0x001F));
        assert_eq!(px(&p, 8, 0), rgb(0x1111));
    }

    #[test]
    fn window_x2_less_than_x1_extends_right() {
        let mut p = ppu_mode(0, 0x0100 | 0x2000);
        full_bg0(&mut p, 0);
        pal(&mut p, 0, 0x1111);
        pal(&mut p, 1, 0x001F);
        p.io_write16(0x40, (10 << 8) | 5); // x2 < x1
        p.io_write16(0x44, 160);
        p.io_write16(0x48, 0x01);
        p.io_write16(0x4A, 0x00);
        p.render_scanline(0);
        assert_eq!(px(&p, 9, 0), rgb(0x1111));
        assert_eq!(px(&p, 10, 0), rgb(0x001F));
        assert_eq!(px(&p, 239, 0), rgb(0x001F));
    }

    #[test]
    fn obj_window_gates_bg() {
        let mut p = ppu_mode(0, 0x0100 | 0x1000 | 0x8000);
        full_bg0(&mut p, 0);
        pal(&mut p, 0, 0x1111);
        pal(&mut p, 1, 0x001F);
        opal(&mut p, 1, 0x03E0);
        solid_obj_tile(&mut p, 1);
        obj(&mut p, 0, 2 << 10, 0, 1); // objwin-mode sprite at x 0..7
        p.io_write16(0x4A, 0x01 << 8); // objwin: bg0; outside: nothing
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F)); // bg shows, not obj color
        assert_eq!(px(&p, 8, 0), rgb(0x1111));
    }

    #[test]
    fn alpha_blend_bg_over_bg() {
        let mut p = ppu_mode(0, 0x0300);
        p.io_write16(0x08, 8 << 8); // bg0 prio 0
        p.io_write16(0x0A, (9 << 8) | 1); // bg1 prio 1
        solid_tile4(&mut p, 0, 1);
        pal(&mut p, 1, 0x001F); // bg0: red 31
        pal(&mut p, 17, 0x7C00); // bg1: blue 31 via palbank 1
        for i in 0..32 * 20 {
            map16(&mut p, 0x4800 + i * 2, 0x1000);
        }
        p.io_write16(0x50, (1 << 6) | 1 | (1 << 9)); // mode 1, bg0 -> bg1
        p.io_write16(0x52, 8 | (8 << 8));
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(15 | 15 << 10));
        p.io_write16(0x52, 31 | (31 << 8)); // EVA/EVB cap at 16
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(31 | 31 << 10));
    }

    #[test]
    fn semi_transparent_obj_forces_blend() {
        let mut p = ppu_mode(0, 0x1100);
        full_bg0(&mut p, 0);
        pal(&mut p, 1, 0x7C00);
        opal(&mut p, 1, 0x001F);
        solid_obj_tile(&mut p, 1);
        obj(&mut p, 0, 1 << 10, 0, 1); // semi-transparent mode
        p.io_write16(0x50, 1 << 8); // mode 0, bg0 as 2nd target only
        p.io_write16(0x52, 8 | (8 << 8));
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(15 | 15 << 10));
        assert_eq!(px(&p, 8, 0), rgb(0x7C00)); // bg alone unblended
        // Window with blend bit clear suppresses semi-obj blending.
        p.io_write16(0x00, 0x1100 | 0x2000);
        p.io_write16(0x40, 240);
        p.io_write16(0x44, 160);
        p.io_write16(0x48, 0x11); // bg0 + obj, no blend bit
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F));
    }

    #[test]
    fn brightness_effects() {
        let mut p = ppu_mode(0, 0x0100);
        full_bg0(&mut p, 0);
        pal(&mut p, 1, 15);
        p.io_write16(0x50, (2 << 6) | 1); // brighten bg0
        p.io_write16(0x54, 8);
        p.render_scanline(0);
        // r: 15 + 16*8/16 = 23; g/b: 0 + 31*8/16 = 15.
        assert_eq!(px(&p, 0, 0), rgb(23 | 15 << 5 | 15 << 10));
        p.io_write16(0x50, (3 << 6) | 1); // darken
        p.io_write16(0x54, 16);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0));
        p.io_write16(0x50, (2 << 6) | 0x20); // brighten backdrop only
        p.io_write16(0x54, 16);
        pal(&mut p, 0, 0x1111);
        p.io_write16(0x00, 0); // no layers
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x7FFF));
    }

    #[test]
    fn bg_mosaic() {
        let mut p = ppu_mode(0, 0x0100);
        p.io_write16(0x08, (8 << 8) | 0x40);
        p.io_write16(0x4C, 3 | (3 << 4)); // 4x4 blocks
        // Tile 1 row0: px0=1 then 2s; row1: all 3.
        p.vram[32] = 0x21;
        p.vram[33] = 0x22;
        p.vram[34] = 0x22;
        p.vram[35] = 0x22;
        p.vram[36..40].fill(0x33);
        pal(&mut p, 1, 0x001F);
        pal(&mut p, 2, 0x03E0);
        pal(&mut p, 3, 0x7C00);
        map16(&mut p, 0x4000, 1);
        p.render_scanline(0);
        for x in 0..4 {
            assert_eq!(px(&p, x, 0), rgb(0x001F)); // stretched px0
        }
        assert_eq!(px(&p, 4, 0), rgb(0x03E0));
        p.render_scanline(1); // vertical: line 1 samples line 0
        assert_eq!(px(&p, 0, 1), rgb(0x001F));
    }

    #[test]
    fn bg_priority_tie_ascending() {
        let mut p = ppu_mode(0, 0x0300);
        p.io_write16(0x08, 8 << 8);
        p.io_write16(0x0A, 9 << 8); // same priority 0
        solid_tile4(&mut p, 0, 1);
        pal(&mut p, 1, 0x001F);
        pal(&mut p, 17, 0x7C00);
        map16(&mut p, 0x4800, 0x1000);
        p.render_scanline(0);
        assert_eq!(px(&p, 0, 0), rgb(0x001F)); // bg0 wins
    }

    #[test]
    fn hblank_flag_irq_and_render() {
        let mut p = Ppu::new();
        p.io_write16(0x00, 0);
        p.io_write16(0x04, 0x10);
        pal(&mut p, 0, 0x001F);
        let (mut i, mut d) = (0u16, dma());
        p.tick(1005, &mut i, &mut d);
        assert_eq!(p.io_read16(4) & 2, 0);
        assert_eq!(i, 0);
        assert_eq!(px(&p, 0, 0), [0, 0, 0]);
        p.tick(1, &mut i, &mut d);
        assert_eq!(p.io_read16(4) & 2, 2);
        assert_eq!(i, 2);
        assert_eq!(px(&p, 0, 0), rgb(0x001F)); // line rendered at hblank
        // HBlank IRQ also fires during vblank lines.
        i = 0;
        p.tick(1232 * 200, &mut i, &mut d);
        assert_eq!(i & 2, 2);
    }

    #[test]
    fn vblank_events_and_flags() {
        let mut p = Ppu::new();
        p.io_write16(0x04, 0x08);
        let (mut i, mut d) = (0u16, dma());
        p.tick(1232 * 160 - 1, &mut i, &mut d);
        assert!(!p.frame_done);
        assert_eq!(i & 1, 0);
        p.tick(1, &mut i, &mut d);
        assert!(p.frame_done);
        assert_eq!(i & 1, 1);
        assert_eq!(p.io_read16(6), 160);
        assert_eq!(p.io_read16(4) & 1, 1);
        p.tick(1232 * 67, &mut i, &mut d);
        assert_eq!(p.io_read16(6), 227);
        assert_eq!(p.io_read16(4) & 1, 0); // flag clears on line 227
        p.tick(1232, &mut i, &mut d);
        assert_eq!(p.io_read16(6), 0);
    }

    #[test]
    fn vcount_match_irq_and_flag() {
        let mut p = Ppu::new();
        p.io_write16(0x04, 0x20 | (5 << 8));
        let (mut i, mut d) = (0u16, dma());
        p.tick(1232 * 5 - 1, &mut i, &mut d);
        assert_eq!(i & 4, 0);
        p.tick(1, &mut i, &mut d);
        assert_eq!(i & 4, 4);
        assert_eq!(p.io_read16(4) & 4, 4);
        p.tick(1232, &mut i, &mut d);
        assert_eq!(p.io_read16(4) & 4, 0);
    }

    #[test]
    fn affine_ref_advances_and_reloads() {
        let mut p = Ppu::new();
        p.io_write16(0x00, 2);
        p.io_write16(0x22, 0x40); // BG2PB
        p.io_write16(0x28, 0x100);
        p.io_write16(0x2A, 0);
        assert_eq!(p.bgx[0], 0x100);
        let (mut i, mut d) = (0u16, dma());
        p.tick(1232, &mut i, &mut d);
        assert_eq!(p.bgx[0], 0x140); // +PB after rendered line
        p.tick(1232 * 227, &mut i, &mut d);
        assert_eq!(p.bgx[0], 0x100); // reloaded at frame start
    }

    #[test]
    fn affine_ref_sign_extends() {
        let mut p = Ppu::new();
        p.io_write16(0x2C, 0);
        p.io_write16(0x2E, 0x0800); // bit 27 set
        assert_eq!(p.bgy[0], 0xF800_0000u32 as i32);
        p.io_write16(0x3A, 0xFFFF); // BG3X high: only low 12 bits kept
        assert_eq!(p.bgx[1], 0xFFFF_0000u32 as i32);
    }

    #[test]
    fn register_read_masks() {
        let mut p = Ppu::new();
        p.io_write16(0x04, 0xFFFF);
        assert_eq!(p.io_read16(4), 0xFF38);
        p.io_write16(0x08, 0xABCD);
        assert_eq!(p.io_read16(8), 0xABCD);
        p.io_write16(0x48, 0xFFFF);
        assert_eq!(p.io_read16(0x48), 0x3F3F);
        p.io_write16(0x52, 0xFFFF);
        assert_eq!(p.io_read16(0x52), 0x1F1F);
        assert_eq!(p.io_read16(0x10), 0); // write-only reads 0
        assert_eq!(p.io_read16(0x54), 0);
    }
}
