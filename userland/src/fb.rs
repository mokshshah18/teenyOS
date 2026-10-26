// Framebuffer output and drawing: a back buffer in RAM, clipped drawing
// primitives, and text in the kernel's own console font.

use crate::sys;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.empty() {
            return *o;
        }
        if o.empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// The console font, fetched from the kernel so we don't ship one.
pub struct Font {
    pub w: i32,
    pub h: i32,
    data: Vec<u8>, // 32 bytes per glyph, one byte per row
}

impl Font {
    pub fn from_console(tty: i32) -> Option<Font> {
        let mut data = vec![0u8; 512 * 32];
        // struct console_font_op { op, flags, width, height, charcount, data* }
        let mut op = [0u32; 8];
        op[0] = 1; // KD_FONT_OP_GET
        op[2] = 8;
        op[3] = 32;
        op[4] = 512;
        let p = data.as_mut_ptr() as u64;
        op[6] = p as u32;
        op[7] = (p >> 32) as u32;
        if sys::ioctl(tty, sys::KDFONTOP, op.as_mut_ptr() as usize) < 0 || op[2] != 8 || op[3] == 0 {
            return None;
        }
        Some(Font { w: 8, h: op[3] as i32, data })
    }
    fn glyph(&self, c: u8) -> &[u8] {
        let i = c as usize * 32;
        &self.data[i..i + self.h as usize]
    }
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    pub clip: Rect,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Canvas {
        Canvas { w, h, px: vec![0; (w * h) as usize], clip: Rect::new(0, 0, w, h) }
    }

    pub fn fill(&mut self, r: Rect, c: u32) {
        let r = r.intersect(&self.clip);
        if r.empty() {
            return;
        }
        for y in r.y..r.y + r.h {
            let row = (y * self.w) as usize;
            self.px[row + r.x as usize..row + (r.x + r.w) as usize].fill(c);
        }
    }

    /// A filled rectangle with its corners clipped off.
    pub fn round(&mut self, r: Rect, c: u32) {
        self.fill(Rect::new(r.x + 2, r.y, r.w - 4, 1), c);
        self.fill(Rect::new(r.x + 1, r.y + 1, r.w - 2, 1), c);
        self.fill(Rect::new(r.x, r.y + 2, r.w, r.h - 4), c);
        self.fill(Rect::new(r.x + 1, r.y + r.h - 2, r.w - 2, 1), c);
        self.fill(Rect::new(r.x + 2, r.y + r.h - 1, r.w - 4, 1), c);
    }

    /// Vertical gradient from `top` to `bot`.
    pub fn gradient(&mut self, r: Rect, top: u32, bot: u32) {
        let lerp = |a: u32, b: u32, t: i32, n: i32| -> u32 {
            let mut out = 0;
            for s in [16, 8, 0] {
                let (ca, cb) = ((a >> s & 255) as i32, (b >> s & 255) as i32);
                out |= ((ca + (cb - ca) * t / n.max(1)) as u32) << s;
            }
            out
        };
        let c = r.intersect(&self.clip);
        for y in c.y..c.y + c.h {
            let col = lerp(top, bot, y - r.y, r.h);
            self.fill(Rect::new(c.x, y, c.w, 1), col);
        }
    }

    /// Draw text; `bg` of None leaves the background showing through.
    /// Returns the x just past the last character.
    pub fn text(&mut self, f: &Font, x: i32, y: i32, s: &[u8], fg: u32, bg: Option<u32>) -> i32 {
        let mut cx = x;
        for &ch in s {
            self.glyph(f, cx, y, ch, fg, bg);
            cx += f.w;
        }
        cx
    }

    pub fn glyph(&mut self, f: &Font, x: i32, y: i32, ch: u8, fg: u32, bg: Option<u32>) {
        let cell = Rect::new(x, y, f.w, f.h);
        let vis = cell.intersect(&self.clip);
        if vis.empty() {
            return;
        }
        if let Some(b) = bg {
            self.fill(vis, b);
        }
        let g = f.glyph(ch);
        for gy in vis.y - y..vis.y - y + vis.h {
            let bits = g[gy as usize];
            if bits == 0 {
                continue;
            }
            let row = ((y + gy) * self.w) as usize;
            for gx in vis.x - x..vis.x - x + vis.w {
                if bits & (0x80 >> gx) != 0 {
                    self.px[row + (x + gx) as usize] = fg;
                }
            }
        }
    }

    /// Bitmap from strings: '#' = `c`, '.' = `edge`, anything else clear.
    /// Each source pixel becomes a `scale` x `scale` block.
    pub fn bitmap(&mut self, x: i32, y: i32, rows: &[&[u8]], scale: i32, c: u32, edge: u32) {
        for (j, row) in rows.iter().enumerate() {
            for (i, &p) in row.iter().enumerate() {
                let col = match p {
                    b'#' => c,
                    b'.' => edge,
                    _ => continue,
                };
                self.fill(Rect::new(x + i as i32 * scale, y + j as i32 * scale, scale, scale), col);
            }
        }
    }
}

/// The screen: /dev/fb0 mapped into memory. On very large screens every
/// canvas pixel becomes a 2x2 block, so text stays readable on 4K panels.
pub struct Screen {
    pub w: i32,
    pub h: i32,
    scale: i32,
    bpp: u32,
    stride: usize,
    shifts: [u32; 3], // red, green, blue bit offsets
    lens: [u32; 3],
    mem: *mut u8,
}

impl Screen {
    pub fn open() -> Option<Screen> {
        let fd = sys::open(b"/dev/fb0", sys::O_RDWR, 0);
        if fd < 0 {
            return None;
        }
        let mut var = [0u32; 40]; // struct fb_var_screeninfo
        let mut fix = [0u32; 20]; // struct fb_fix_screeninfo
        if sys::ioctl(fd, 0x4600, var.as_mut_ptr() as usize) < 0 || sys::ioctl(fd, 0x4602, fix.as_mut_ptr() as usize) < 0 {
            return None;
        }
        let (w, h, bpp) = (var[0] as i32, var[1] as i32, var[6]);
        let smem_len = fix[6] as usize;
        let stride = fix[12] as usize;
        if !(bpp == 16 || bpp == 24 || bpp == 32) || w == 0 || h == 0 {
            return None;
        }
        let mem = sys::mmap_shared(fd, smem_len);
        if mem.is_null() {
            return None;
        }
        let scale = if w >= 2400 { 2 } else { 1 };
        Some(Screen {
            w: w / scale,
            h: h / scale,
            scale,
            bpp,
            stride,
            shifts: [var[8], var[11], var[14]],
            lens: [var[9], var[12], var[15]],
            mem,
        })
    }

    /// Copy part of the canvas to the screen, converting the pixel format.
    pub fn blit(&self, cv: &Canvas, r: Rect) {
        let r = r.intersect(&Rect::new(0, 0, self.w.min(cv.w), self.h.min(cv.h)));
        if r.empty() {
            return;
        }
        let native = self.bpp == 32 && self.shifts == [16, 8, 0];
        let s = self.scale as usize;
        let mut wide: Vec<u32> = Vec::new();
        for (y, sy) in (r.y..r.y + r.h).flat_map(|y| (0..s).map(move |k| (y, k))) {
            let mut src = &cv.px[(y * cv.w + r.x) as usize..(y * cv.w + r.x + r.w) as usize];
            if s > 1 {
                wide.clear();
                wide.extend(src.iter().flat_map(|&c| core::iter::repeat(c).take(s)));
                src = &wide;
            }
            let row = unsafe { self.mem.add((y as usize * s + sy) * self.stride) };
            let x0 = r.x as usize * s;
            if native {
                unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), (row as *mut u32).add(x0), src.len()) };
                continue;
            }
            for (i, &c) in src.iter().enumerate() {
                let mut v = 0u32;
                for k in 0..3 {
                    let comp = c >> (16 - 8 * k) & 255;
                    v |= (comp >> (8 - self.lens[k])) << self.shifts[k];
                }
                let x = x0 + i;
                unsafe {
                    match self.bpp {
                        32 => *(row as *mut u32).add(x) = v,
                        24 => {
                            let p = row.add(x * 3);
                            *p = v as u8;
                            *p.add(1) = (v >> 8) as u8;
                            *p.add(2) = (v >> 16) as u8;
                        }
                        _ => *(row as *mut u16).add(x) = v as u16,
                    }
                }
            }
        }
    }
}
