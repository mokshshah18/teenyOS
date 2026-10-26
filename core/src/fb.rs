// Framebuffer drawing: an off-screen canvas with clipped primitives and text,
// blitted to the firmware's graphics output. The Canvas code is shared in
// spirit with the Linux edition; Screen here talks to UEFI GOP.

use crate::font;
use crate::uefi::Gop;
use alloc::vec;
use alloc::vec::Vec;

pub use font::{FH, FW};

#[derive(Clone, Copy, PartialEq)]
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
        if r.w < 4 || r.h < 4 {
            self.fill(r, c);
            return;
        }
        self.fill(Rect::new(r.x + 2, r.y, r.w - 4, 1), c);
        self.fill(Rect::new(r.x + 1, r.y + 1, r.w - 2, 1), c);
        self.fill(Rect::new(r.x, r.y + 2, r.w, r.h - 4), c);
        self.fill(Rect::new(r.x + 1, r.y + r.h - 2, r.w - 2, 1), c);
        self.fill(Rect::new(r.x + 2, r.y + r.h - 1, r.w - 4, 1), c);
    }

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

    pub fn text(&mut self, x: i32, y: i32, s: &[u8], fg: u32, bg: Option<u32>) -> i32 {
        let mut cx = x;
        for &ch in s {
            self.glyph(cx, y, ch, fg, bg);
            cx += FW;
        }
        cx
    }

    pub fn glyph(&mut self, x: i32, y: i32, ch: u8, fg: u32, bg: Option<u32>) {
        let cell = Rect::new(x, y, FW, FH);
        let vis = cell.intersect(&self.clip);
        if vis.empty() {
            return;
        }
        if let Some(b) = bg {
            self.fill(vis, b);
        }
        let g = font::glyph(ch);
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

    /// Text scaled up by an integer factor, for the wordmark.
    pub fn text_big(&mut self, x: i32, y: i32, s: &[u8], scale: i32, col: u32) {
        for (k, &ch) in s.iter().enumerate() {
            let g = font::glyph(ch);
            let gx = x + k as i32 * FW * scale;
            for (py, bits) in g.iter().enumerate() {
                for px in 0..FW {
                    if bits & (0x80 >> px) != 0 {
                        self.fill(Rect::new(gx + px * scale, y + py as i32 * scale, scale, scale), col);
                    }
                }
            }
        }
    }

    /// Bitmap icon from strings: '#' = fill, '.' = edge, else clear.
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

/// The physical screen. We push pixels through the firmware's own Blt, which
/// is portable across pixel formats and reliably drives the display. Our
/// canvas pixel 0x00RRGGBB is, in memory, exactly an EFI blt pixel
/// (Blue, Green, Red, Reserved), so no conversion is needed.
pub struct Screen {
    pub w: i32,
    pub h: i32,
    gop: *mut u8,
}

impl Screen {
    pub fn new(g: &Gop) -> Screen {
        Screen { w: g.w, h: g.h, gop: g.handle }
    }

    pub fn blit(&self, cv: &Canvas, r: Rect) {
        let r = r.intersect(&Rect::new(0, 0, self.w.min(cv.w), self.h.min(cv.h)));
        if r.empty() {
            return;
        }
        for y in r.y..r.y + r.h {
            let row = (y * cv.w) as usize;
            let mut x = r.x;
            while x < r.x + r.w {
                let c = cv.px[row + x as usize];
                let mut run = 1;
                while x + run < r.x + r.w && cv.px[row + (x + run) as usize] == c {
                    run += 1;
                }
                crate::uefi::gop_fill(self.gop, c, x, y, run, 1);
                x += run;
            }
        }
    }
}
