// The desktop: a small window manager drawn on the firmware framebuffer, with
// a mouse from the firmware pointer, draggable windows, a taskbar and start
// menu, and terminal windows each backed by a shell green thread.

use crate::fb::{rgb, Canvas, Rect, Screen};
use crate::util::num;
use crate::{proc, sched, shell, sysb as sys, tty, uefi};
use alloc::vec;
use alloc::vec::Vec;

const TITLE_H: i32 = 24;
const BAR_H: i32 = 30;

const DESK_TOP: u32 = rgb(0x24, 0x3b, 0x55);
const DESK_BOT: u32 = rgb(0x14, 0x6c, 0x7a);
const BAR: u32 = rgb(0x16, 0x18, 0x22);
const ACCENT: u32 = rgb(0x3d, 0x8b, 0xfd);
const WHITE: u32 = rgb(0xff, 0xff, 0xff);
const INK: u32 = rgb(0x1d, 0x22, 0x2b);
const WIN_BG: u32 = rgb(0xf4, 0xf5, 0xf7);

const CURSOR: &[&[u8]] = &[
    b".", b"..", b".#.", b".##.", b".###.", b".####.", b".#####.", b".######.",
    b".#######.", b".########.", b".#####....", b".##.##.", b".#. .##.", b"..  .##.",
    b"    .##.", b"     ..",
];

enum App {
    Term { tty: u32, thread: u32 },
    About,
}

struct Win {
    r: Rect,
    title: Vec<u8>,
    app: App,
    min: bool,
}

impl Win {
    fn content(&self) -> Rect {
        Rect::new(self.r.x + 1, self.r.y + TITLE_H, self.r.w - 2, self.r.h - TITLE_H - 1)
    }
    fn close_box(&self) -> Rect {
        Rect::new(self.r.x + self.r.w - 22, self.r.y + 4, 16, 16)
    }
}

const MENU: &[&[u8]] = &[b"New terminal", b"About teenyOS", b"", b"Power off", b"Restart"];

pub struct Desk {
    scr: Screen,
    cv: Canvas,
    wins: Vec<Win>,
    mx: i32,
    my: i32,
    buttons: u8,
    drag: Option<(i32, i32)>,
    menu: bool,
    spawn_n: i32,
    clock: Vec<u8>,
    full_redraw: bool,
}

impl Desk {
    fn bar(&self) -> Rect {
        Rect::new(0, self.scr.h - BAR_H, self.scr.w, BAR_H)
    }
    fn focused(&self) -> Option<usize> {
        self.wins.iter().rposition(|w| !w.min)
    }
    fn menu_rect(&self) -> Rect {
        let h = MENU.len() as i32 * 22 + 12;
        Rect::new(4, self.scr.h - BAR_H - h - 2, 190, h)
    }

    fn new_terminal(&mut self) {
        let (fw, fh) = (crate::fb::FW, crate::fb::FH);
        let cols = 70usize.min(((self.scr.w - 120) / fw) as usize);
        let rows = 22usize.min(((self.scr.h - 160) / fh) as usize);
        let tid = tty::create(cols, rows);
        let thread = sched::spawn(proc::Proc::new(tid), shell::main);
        let w = cols as i32 * fw + 8;
        let h = rows as i32 * fh + 8 + TITLE_H;
        let off = (self.spawn_n % 6) * 26;
        self.spawn_n += 1;
        let x = (60 + off).min(self.scr.w - w - 10);
        let y = (40 + off).min(self.scr.h - BAR_H - h - 10);
        self.wins.push(Win { r: Rect::new(x, y, w, h), title: b"Terminal".to_vec(), app: App::Term { tty: tid, thread }, min: false });
        self.full_redraw = true;
    }

    fn open_about(&mut self) {
        let (w, h) = (440, 220);
        let x = (self.scr.w - w) / 2;
        let y = (self.scr.h - h) / 2 - 40;
        self.wins.push(Win { r: Rect::new(x, y, w, h), title: b"About teenyOS".to_vec(), app: App::About, min: false });
        self.full_redraw = true;
    }

    fn close(&mut self, i: usize) {
        if let App::Term { tty, thread } = self.wins[i].app {
            sched::kill_thread(thread);
            tty::destroy(tty);
        }
        self.wins.remove(i);
        self.full_redraw = true;
    }

    fn raise(&mut self, i: usize) {
        let w = self.wins.remove(i);
        self.wins.push(w);
        self.full_redraw = true;
    }

    fn click(&mut self, x: i32, y: i32) {
        if self.menu {
            self.menu = false;
            self.full_redraw = true;
            let m = self.menu_rect();
            if m.contains(x, y) {
                let item = (y - m.y - 6) / 22;
                match item {
                    0 => self.new_terminal(),
                    1 => self.open_about(),
                    3 => {
                        crate::persist::save();
                        uefi::reset(true);
                    }
                    4 => {
                        crate::persist::save();
                        uefi::reset(false);
                    }
                    _ => {}
                }
            }
            return;
        }
        if self.bar().contains(x, y) {
            if x < 100 {
                self.menu = true;
                self.full_redraw = true;
            } else {
                // Taskbar window buttons.
                let mut bx = 110;
                for i in 0..self.wins.len() {
                    if x >= bx && x < bx + 150 {
                        if self.focused() == Some(i) && !self.wins[i].min {
                            self.wins[i].min = true;
                        } else {
                            self.wins[i].min = false;
                            self.raise(i);
                        }
                        self.full_redraw = true;
                        return;
                    }
                    bx += 156;
                }
            }
            return;
        }
        if let Some(i) = self.wins.iter().rposition(|w| !w.min && w.r.contains(x, y)) {
            if Some(i) != self.focused() {
                self.raise(i);
                return;
            }
            let i = self.wins.len() - 1;
            if self.wins[i].close_box().contains(x, y) {
                self.close(i);
            } else if y < self.wins[i].r.y + TITLE_H {
                self.drag = Some((x - self.wins[i].r.x, y - self.wins[i].r.y));
            }
        }
    }

    fn mouse(&mut self, nx: i32, ny: i32, buttons: u8) {
        self.mx = nx.clamp(0, self.scr.w - 1);
        self.my = ny.clamp(0, self.scr.h - 1);
        if let Some((ox, oy)) = self.drag {
            if let Some(i) = self.focused() {
                self.wins[i].r.x = (self.mx - ox).clamp(-self.wins[i].r.w + 60, self.scr.w - 60);
                self.wins[i].r.y = (self.my - oy).clamp(0, self.scr.h - BAR_H - TITLE_H);
                self.full_redraw = true;
            }
        }
        let pressed = buttons & 1 != 0 && self.buttons & 1 == 0;
        let released = buttons & 1 == 0 && self.buttons & 1 != 0;
        self.buttons = buttons;
        if released {
            self.drag = None;
        }
        if pressed {
            self.click(self.mx, self.my);
        }
    }

    fn cycle_focus(&mut self) {
        // Move the current top window to the bottom so the next shows.
        if self.wins.len() > 1 {
            if let Some(i) = self.focused() {
                let w = self.wins.remove(i);
                self.wins.insert(0, w);
                for win in self.wins.iter_mut() {
                    win.min = false;
                }
                self.full_redraw = true;
            }
        }
    }

    /// Handle a window-manager shortcut (function key). Returns true if consumed.
    fn wm_key(&mut self, scan: u16) -> bool {
        match scan {
            0x0b => {
                self.new_terminal();
                true
            }
            0x0c => {
                self.cycle_focus();
                true
            }
            0x0d => {
                self.open_about();
                true
            }
            0x0e => {
                if let Some(i) = self.focused() {
                    self.close(i);
                }
                true
            }
            _ => false,
        }
    }

    fn keys(&mut self, bytes: &[u8]) {
        if let Some(i) = self.focused() {
            if let App::Term { tty, .. } = self.wins[i].app {
                tty::push_input(tty, bytes);
            }
        }
    }

    fn paint(&mut self) {
        let (w, h) = (self.scr.w, self.scr.h);
        self.cv.clip = Rect::new(0, 0, w, h);
        self.cv.gradient(Rect::new(0, 0, w, h), DESK_TOP, DESK_BOT);
        let wm = b"teenyOS core";
        self.cv.text_big(w - wm.len() as i32 * crate::fb::FW * 3 - 30, h - BAR_H - crate::fb::FH * 3 - 24, wm, 3, rgb(0x5f, 0xb0, 0xc0));
        let top = self.wins.iter().rposition(|w| !w.min);
        for i in 0..self.wins.len() {
            if self.wins[i].min {
                continue;
            }
            let r = self.wins[i].r;
            let focus = Some(i) == top;
            self.cv.fill(Rect::new(r.x + 3, r.y + 3, r.w, r.h), rgb(0x0a, 0x14, 0x1e));
            self.cv.fill(r, rgb(0x0b, 0x0d, 0x12));
            self.cv.fill(Rect::new(r.x + 1, r.y + 1, r.w - 2, TITLE_H - 1), if focus { ACCENT } else { rgb(0x4a, 0x50, 0x5e) });
            self.cv.text(r.x + 8, r.y + (TITLE_H - crate::fb::FH) / 2, &self.wins[i].title, WHITE, None);
            let cb = self.wins[i].close_box();
            self.cv.round(cb, rgb(0xe5, 0x48, 0x4d));
            self.cv.text(cb.x + 4, cb.y + 1, b"x", WHITE, None);
            let c = self.wins[i].content();
            let saved = self.cv.clip;
            self.cv.clip = saved.intersect(&c);
            self.draw_app(i, c, focus);
            self.cv.clip = saved;
        }
        self.draw_bar(top);
        if self.menu {
            self.draw_menu();
        }
        self.cv.bitmap(self.mx, self.my, CURSOR, 1, WHITE, rgb(0, 0, 0));
    }

    fn draw_app(&mut self, i: usize, c: Rect, focus: bool) {
        match self.wins[i].app {
            App::Term { tty: id, .. } => {
                self.cv.fill(c, tty::palette(0));
                let cv = &mut self.cv;
                tty::render(id, |t| {
                    for y in 0..t.rows {
                        let py = c.y + 2 + y as i32 * crate::fb::FH;
                        for x in 0..t.cols {
                            let cell = t.cells[y * t.cols + x];
                            let px = c.x + 4 + x as i32 * crate::fb::FW;
                            let bg = if cell.bg == 0 { None } else { Some(tty::palette(cell.bg)) };
                            if cell.ch != b' ' || bg.is_some() {
                                cv.glyph(px, py, cell.ch, tty::palette(cell.fg), bg);
                            }
                        }
                    }
                    if t.cursor_on && focus {
                        let x = c.x + 4 + t.cx.min(t.cols - 1) as i32 * crate::fb::FW;
                        let y = c.y + 2 + t.cy as i32 * crate::fb::FH;
                        cv.fill(Rect::new(x, y + crate::fb::FH - 2, crate::fb::FW, 2), tty::palette(15));
                    }
                });
            }
            App::About => {
                self.cv.fill(c, WIN_BG);
                self.cv.text_big(c.x + 20, c.y + 18, b"teenyOS core", 3, ACCENT);
                let lines: [&[u8]; 5] = [
                    b"A desktop OS with no kernel.",
                    b"It runs as a UEFI application on the firmware,",
                    b"so graphics, disk and the network are the",
                    b"firmware's code -- teenyOS ships no drivers.",
                    b"Everything else is one Rust program.",
                ];
                let mut y = c.y + 18 + crate::fb::FH * 3 + 16;
                for l in lines {
                    self.cv.text(c.x + 22, y, l, INK, None);
                    y += crate::fb::FH + 5;
                }
            }
        }
    }

    fn draw_bar(&mut self, top: Option<usize>) {
        let (w, h) = (self.scr.w, self.scr.h);
        let bar = Rect::new(0, h - BAR_H, w, BAR_H);
        self.cv.fill(bar, BAR);
        self.cv.fill(Rect::new(0, bar.y, w, 1), rgb(0x3a, 0x40, 0x52));
        let sb = Rect::new(6, bar.y + 4, 90, BAR_H - 8);
        self.cv.round(sb, if self.menu { ACCENT } else { rgb(0x2c, 0x31, 0x42) });
        self.cv.round(Rect::new(sb.x + 7, sb.y + 5, 11, 11), rgb(0x6f, 0xc3, 0xd0));
        self.cv.text(sb.x + 26, sb.y + (sb.h - crate::fb::FH) / 2, b"teeny", WHITE, None);
        let mut bx = 110;
        for (i, win) in self.wins.iter().enumerate() {
            let b = Rect::new(bx, bar.y + 4, 150, BAR_H - 8);
            self.cv.round(b, if Some(i) == top { rgb(0x3a, 0x42, 0x58) } else { rgb(0x24, 0x28, 0x34) });
            let t = &win.title[..win.title.len().min(16)];
            self.cv.text(b.x + 8, b.y + (b.h - crate::fb::FH) / 2, t, if win.min { rgb(0x8a, 0x92, 0xa0) } else { WHITE }, None);
            bx += 156;
        }
        let clk = &self.clock;
        self.cv.text(w - clk.len() as i32 * crate::fb::FW - 12, bar.y + (BAR_H - crate::fb::FH) / 2, clk, rgb(0xd0, 0xd6, 0xe0), None);
    }

    fn draw_menu(&mut self) {
        let m = self.menu_rect();
        self.cv.fill(Rect::new(m.x + 3, m.y + 3, m.w, m.h), rgb(0x05, 0x08, 0x0c));
        self.cv.round(m, rgb(0x20, 0x24, 0x31));
        for (k, item) in MENU.iter().enumerate() {
            let iy = m.y + 6 + k as i32 * 22;
            if item.is_empty() {
                self.cv.fill(Rect::new(m.x + 8, iy + 10, m.w - 16, 1), rgb(0x3a, 0x40, 0x52));
                continue;
            }
            if Rect::new(m.x + 3, iy, m.w - 6, 22).contains(self.mx, self.my) {
                self.cv.round(Rect::new(m.x + 3, iy, m.w - 6, 22), ACCENT);
            }
            self.cv.text(m.x + 14, iy + 4, item, WHITE, None);
        }
    }

    fn tick(&mut self) {
        let t = sys::now() % 86400;
        let two = |n: u64| [b'0' + (n / 10 % 10) as u8, b'0' + (n % 10) as u8];
        let mut c = num(t / 3600);
        c.push(b':');
        c.extend_from_slice(&two(t / 60 % 60));
        self.clock = c;
    }
}

pub fn run(scr: Screen) -> ! {
    let (w, h) = (scr.w, scr.h);
    let mut d = Desk {
        scr,
        cv: Canvas::new(w, h),
        wins: Vec::new(),
        mx: w / 2,
        my: h / 2,
        buttons: 0,
        drag: None,
        menu: false,
        spawn_n: 0,
        clock: Vec::new(),
        full_redraw: true,
    };
    d.tick();
    d.new_terminal();
    let pointer = uefi::open_pointer();
    let key_ev = uefi::key_event();
    let (pev, pn) = pointer.events();
    let timer = uefi::create_timer();
    let mut last_second = sys::now();
    // Absolute pointers report a fraction of the screen; track it.
    let (mut ax, mut ay) = (w / 2, h / 2);

    loop {
        sys::tick_ms(16);
        let mut activity = false;

        while let Some((scan, uni)) = uefi::read_key() {
            if d.wm_key(scan) {
                activity = true;
                continue;
            }
            let b = crate::keymap(scan, uni);
            if !b.is_empty() {
                d.keys(&b);
                activity = true;
            }
        }
        // Pointer: coalesce all queued movement, apply once.
        let mut moved = false;
        let mut btn = d.buttons;
        while let Some(m) = pointer.poll() {
            moved = true;
            match m {
                uefi::Move::Abs(fx, fy, b) => {
                    ax = (fx * w as f32) as i32;
                    ay = (fy * h as f32) as i32;
                    btn = b as u8;
                }
                uefi::Move::Rel(dx, dy, b) => {
                    ax = (ax + (dx as i32) / 2).clamp(0, w - 1);
                    ay = (ay + (dy as i32) / 2).clamp(0, h - 1);
                    btn = b as u8;
                }
            }
        }
        if moved || btn != d.buttons {
            d.mouse(ax, ay, btn);
            activity = true;
        }

        let worked = sched::pump();

        // Close terminal windows whose shell has exited.
        let mut i = 0;
        while i < d.wins.len() {
            let dead = matches!(d.wins[i].app, App::Term { thread, .. } if !sched::alive(thread));
            if dead {
                d.close(i);
            } else {
                i += 1;
            }
        }

        let any_dirty = d.wins.iter().any(|w| matches!(w.app, App::Term { tty, .. } if !w.min && tty::is_dirty(tty)));
        if d.full_redraw || any_dirty || activity {
            d.paint();
            d.scr.blit(&d.cv, Rect::new(0, 0, w, h));
            d.full_redraw = false;
        }

        if sys::now() != last_second {
            last_second = sys::now();
            d.tick();
            d.full_redraw = true;
        }

        if !worked && !activity {
            let mut evs = vec![key_ev, timer];
            evs.extend_from_slice(&pev[..pn]);
            uefi::wait_any(&evs);
        } else {
            uefi::stall(2_000);
        }
    }
}
