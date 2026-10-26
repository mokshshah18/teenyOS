// The desktop: a small window manager drawn straight onto the framebuffer,
// with a taskbar, a start menu, desktop icons, and a few built-in apps.

use crate::fb::{rgb, Canvas, Font, Rect, Screen};
use crate::sys;
use crate::term::Term;
use crate::util::*;
use alloc::vec;
use alloc::vec::Vec;

const TITLE_H: i32 = 26;
const BAR_H: i32 = 34;
const ROW_H: i32 = 22;

const DESK_TOP: u32 = rgb(0x24, 0x3b, 0x55);
const DESK_BOT: u32 = rgb(0x14, 0x6c, 0x7a);
const BAR: u32 = rgb(0x16, 0x18, 0x22);
const BAR_HI: u32 = rgb(0x2c, 0x31, 0x42);
const ACCENT: u32 = rgb(0x3d, 0x8b, 0xfd);
const WIN_BG: u32 = rgb(0xf4, 0xf5, 0xf7);
const WIN_EDGE: u32 = rgb(0x0b, 0x0d, 0x12);
const TITLE_OFF: u32 = rgb(0x4a, 0x50, 0x5e);
const INK: u32 = rgb(0x1d, 0x22, 0x2b);
const MUTED: u32 = rgb(0x6b, 0x72, 0x80);
const WHITE: u32 = rgb(0xff, 0xff, 0xff);
const SEL: u32 = rgb(0xd6, 0xe6, 0xff);

// 16x16 icons, drawn at 2x. '#' is the icon colour, '.' its outline.
const ICON_TERM: &[&[u8]] = &[
    b"................",
    b".##############.",
    b".#............#.",
    b".#.##.........#.",
    b".#..##........#.",
    b".#...##.......#.",
    b".#..##........#.",
    b".#.##..####...#.",
    b".#............#.",
    b".#............#.",
    b".#............#.",
    b".##############.",
    b"................",
    b".....######.....",
    b"...##########...",
    b"................",
];
const ICON_FOLDER: &[&[u8]] = &[
    b"                ",
    b"                ",
    b" ......         ",
    b" .####.         ",
    b" .#####........ ",
    b" .############. ",
    b" .############. ",
    b" .############. ",
    b" .############. ",
    b" .############. ",
    b" .############. ",
    b" .############. ",
    b" .############. ",
    b" .............. ",
    b"                ",
    b"                ",
];
const ICON_CHART: &[&[u8]] = &[
    b"................",
    b".##############.",
    b".#............#.",
    b".#.........##.#.",
    b".#.........##.#.",
    b".#......##.##.#.",
    b".#......##.##.#.",
    b".#...##.##.##.#.",
    b".#...##.##.##.#.",
    b".#.#####.#####.#",
    b".#.##.##.##.##.#",
    b".#............#.",
    b".##############.",
    b"................",
    b"                ",
    b"                ",
];
const ICON_DRIVE: &[&[u8]] = &[
    b"                ",
    b"                ",
    b"   ..........   ",
    b"   .########.   ",
    b"   .#......#.   ",
    b"   .#......#.   ",
    b"   .########.   ",
    b"  ............  ",
    b"  .##########.  ",
    b"  .##########.  ",
    b"  .######..##.  ",
    b"  .######..##.  ",
    b"  .##########.  ",
    b"  ............  ",
    b"                ",
    b"                ",
];
const ICON_INFO: &[&[u8]] = &[
    b"     ......     ",
    b"   ..######..   ",
    b"  .##########.  ",
    b" .#####..#####. ",
    b" .#####..#####. ",
    b".##############.",
    b".######..######.",
    b".######..######.",
    b".######..######.",
    b".######..######.",
    b" .#####..#####. ",
    b" .############. ",
    b"  .##########.  ",
    b"   ..######..   ",
    b"     ......     ",
    b"                ",
];
const CURSOR: &[&[u8]] = &[
    b".",
    b"..",
    b".#.",
    b".##.",
    b".###.",
    b".####.",
    b".#####.",
    b".######.",
    b".#######.",
    b".########.",
    b".#####.....",
    b".##.##.",
    b".#. .##.",
    b"..  .##.",
    b".    .##.",
    b"     .##.",
    b"      ..",
];

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Terminal,
    Files,
    Monitor,
    Storage,
    About,
}

const LAUNCHERS: &[(Kind, &[u8])] = &[
    (Kind::Terminal, b"Terminal"),
    (Kind::Files, b"Files"),
    (Kind::Monitor, b"Monitor"),
    (Kind::Storage, b"Storage"),
    (Kind::About, b"About"),
];

fn icon_of(k: Kind) -> (&'static [&'static [u8]], u32) {
    match k {
        Kind::Terminal => (ICON_TERM, rgb(0x2b, 0x30, 0x3b)),
        Kind::Files => (ICON_FOLDER, rgb(0xf6, 0xc3, 0x4b)),
        Kind::Monitor => (ICON_CHART, rgb(0x4a, 0xd6, 0x8e)),
        Kind::Storage => (ICON_DRIVE, rgb(0xa8, 0xb4, 0xc8)),
        Kind::About => (ICON_INFO, ACCENT),
    }
}

struct Files {
    dir: Vec<u8>,
    entries: Vec<(Vec<u8>, bool, u64)>,
    sel: usize,
    top: usize,
}

impl Files {
    fn new(dir: &[u8]) -> Files {
        let mut f = Files { dir: dir.to_vec(), entries: Vec::new(), sel: 0, top: 0 };
        f.load();
        f
    }
    fn load(&mut self) {
        let mut e: Vec<(Vec<u8>, bool, u64)> = list_dir(&self.dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n[0] != b'.')
            .map(|n| {
                let st = sys::stat(&join(&self.dir, &n));
                let d = st.as_ref().map_or(false, |s| s.is_dir());
                let size = st.map_or(0, |s| s.size());
                (n, d, size)
            })
            .collect();
        e.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if self.dir != b"/" {
            e.insert(0, (b"..".to_vec(), true, 0));
        }
        self.entries = e;
        self.sel = 0;
        self.top = 0;
    }
    /// Open the selected entry: enter a directory, or return a file path.
    fn open(&mut self) -> Option<Vec<u8>> {
        let (name, is_dir, _) = self.entries.get(self.sel)?.clone();
        if name == b".." {
            self.dir = dirname(&self.dir).to_vec();
            self.load();
            return None;
        }
        let path = join(&self.dir, &name);
        if is_dir {
            self.dir = path;
            self.load();
            None
        } else {
            Some(path)
        }
    }
}

enum App {
    Term(Term),
    Files(Files),
    Monitor(Vec<Vec<u8>>),
    Storage(Vec<Vec<u8>>),
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
        Rect::new(self.r.x + self.r.w - 26, self.r.y + 4, 20, 18)
    }
    fn min_box(&self) -> Rect {
        Rect::new(self.r.x + self.r.w - 50, self.r.y + 4, 20, 18)
    }
}

enum Action {
    Text,
    Reboot,
    Poweroff,
}

const MENU: &[&[u8]] = &[b"Terminal", b"Files", b"Monitor", b"Storage", b"About", b"", b"Exit to console", b"Reboot", b"Power off"];

struct Desk {
    scr: Screen,
    cv: Canvas,
    font: Font,
    wins: Vec<Win>,
    mx: i32,
    my: i32,
    buttons: u8,
    drag: Option<(i32, i32)>,
    menu: bool,
    dirty: Vec<Rect>,
    last_click: (u64, i32, i32),
    clock: Vec<u8>,
    status: Vec<u8>,
    quit: Option<Action>,
    spawn_n: i32,
}

fn persist_status() -> Vec<u8> {
    match read_file(b"/run/persist") {
        Ok(d) if !trim(&d).is_empty() => trim(&d).to_vec(),
        _ => Vec::new(),
    }
}

impl Desk {
    fn full(&self) -> Rect {
        Rect::new(0, 0, self.scr.w, self.scr.h)
    }
    fn bar(&self) -> Rect {
        Rect::new(0, self.scr.h - BAR_H, self.scr.w, BAR_H)
    }
    fn mark(&mut self, r: Rect) {
        let r = r.intersect(&self.full());
        if r.empty() {
            return;
        }
        self.dirty.push(r);
        if self.dirty.len() > 12 {
            let u = self.dirty.iter().fold(Rect::new(0, 0, 0, 0), |a, b| a.union(b));
            self.dirty = vec![u];
        }
    }
    fn menu_rect(&self) -> Rect {
        let h = MENU.len() as i32 * ROW_H + 40;
        Rect::new(4, self.scr.h - BAR_H - h - 4, 200, h)
    }
    fn icon_rect(&self, i: usize) -> Rect {
        Rect::new(18, 18 + i as i32 * 84, 72, 76)
    }
    fn focused(&self) -> Option<usize> {
        self.wins.iter().rposition(|w| !w.min)
    }

    // ------------------------------------------------------------ windows

    fn open(&mut self, k: Kind) {
        self.open_cmd(k, &[]);
    }

    fn open_cmd(&mut self, k: Kind, argv: &[&[u8]]) {
        let (fw, fh) = (self.font.w, self.font.h);
        let (sw, sh) = (self.scr.w, self.scr.h - BAR_H);
        let (w, h, title, app) = match k {
            Kind::Terminal => {
                let cols = (80).min(((sw - 60) / fw) as usize);
                let rows = (24).min(((sh - 80 - TITLE_H) / fh) as usize);
                let Some(t) = Term::spawn(cols, rows, argv) else { return };
                let title = if argv.is_empty() { b"Terminal".to_vec() } else { argv.join(&b' ') };
                (cols as i32 * fw + 10, rows as i32 * fh + 10 + TITLE_H, title, App::Term(t))
            }
            Kind::Files => (520, 420, b"Files".to_vec(), App::Files(Files::new(b"/root"))),
            Kind::Monitor => (460, 400, b"System monitor".to_vec(), App::Monitor(Vec::new())),
            Kind::Storage => (520, 330, b"Storage and persistence".to_vec(), App::Storage(Vec::new())),
            Kind::About => (420, 300, b"About teenyOS".to_vec(), App::About),
        };
        let (w, h) = (w.min(sw - 8), h.min(sh - 8));
        let off = (self.spawn_n % 6) * 28;
        self.spawn_n += 1;
        let x = ((sw - w) / 2 - 70 + off).clamp(0, sw - w);
        let y = ((sh - h) / 2 - 50 + off).clamp(0, sh - h);
        let mut win = Win { r: Rect::new(x, y, w, h), title, app, min: false };
        refresh(&mut win.app);
        self.mark(win.r);
        self.mark(self.bar());
        self.wins.push(win);
    }

    fn raise(&mut self, i: usize) {
        let w = self.wins.remove(i);
        self.mark(w.r);
        self.wins.push(w);
        self.mark(self.bar());
    }

    fn close(&mut self, i: usize) {
        let w = self.wins.remove(i);
        self.mark(w.r);
        self.mark(self.bar());
    }

    fn launch_menu(&mut self, item: usize) {
        match item {
            0..=4 => self.open(LAUNCHERS[item].0),
            6 => self.quit = Some(Action::Text),
            7 => self.quit = Some(Action::Reboot),
            8 => self.quit = Some(Action::Poweroff),
            _ => {}
        }
    }

    // ------------------------------------------------------------ input

    fn mouse(&mut self, dx: i32, dy: i32, buttons: u8) {
        let old = Rect::new(self.mx, self.my, 12, 18);
        self.mx = (self.mx + dx).clamp(0, self.scr.w - 1);
        self.my = (self.my - dy).clamp(0, self.scr.h - 1);
        if dx != 0 || dy != 0 {
            self.mark(old);
            self.mark(Rect::new(self.mx, self.my, 12, 18));
            if self.menu {
                self.mark(self.menu_rect());
            }
        }
        if let Some((ox, oy)) = self.drag {
            if let Some(i) = self.focused() {
                let r = self.wins[i].r;
                let nx = (self.mx - ox).clamp(-r.w + 80, self.scr.w - 80);
                let ny = (self.my - oy).clamp(0, self.scr.h - BAR_H - TITLE_H);
                if nx != r.x || ny != r.y {
                    self.mark(r);
                    self.wins[i].r.x = nx;
                    self.wins[i].r.y = ny;
                    self.mark(self.wins[i].r);
                }
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

    fn click(&mut self, x: i32, y: i32) {
        let now = sys::now_ms();
        let double = now - self.last_click.0 < 450 && (x - self.last_click.1).abs() < 6 && (y - self.last_click.2).abs() < 6;
        self.last_click = (now, x, y);

        if self.menu {
            self.menu = false;
            let m = self.menu_rect();
            self.mark(m);
            if m.contains(x, y) {
                let item = (y - m.y - 36) / ROW_H;
                if item >= 0 && (item as usize) < MENU.len() {
                    self.launch_menu(item as usize);
                }
                return;
            }
        }

        if self.bar().contains(x, y) {
            if x < 110 {
                self.menu = true;
                self.mark(self.menu_rect());
                return;
            }
            let mut bx = 120;
            for i in 0..self.wins.len() {
                if x >= bx && x < bx + 160 {
                    let top = self.focused() == Some(i);
                    if top {
                        self.wins[i].min = true;
                        self.mark(self.wins[i].r);
                    } else {
                        self.wins[i].min = false;
                        self.raise(i);
                    }
                    self.mark(self.bar());
                    return;
                }
                bx += 166;
            }
            return;
        }

        if let Some(i) = self.wins.iter().rposition(|w| !w.min && w.r.contains(x, y)) {
            if Some(i) != self.focused() {
                self.raise(i);
            }
            let i = self.wins.len() - 1;
            let w = &self.wins[i];
            if w.close_box().contains(x, y) {
                self.close(i);
            } else if w.min_box().contains(x, y) {
                self.wins[i].min = true;
                self.mark(self.wins[i].r);
                self.mark(self.bar());
            } else if y < w.r.y + TITLE_H {
                self.drag = Some((x - w.r.x, y - w.r.y));
            } else {
                let c = w.content();
                self.app_click(i, x - c.x, y - c.y, double);
            }
            return;
        }

        for (k, &(kind, _)) in LAUNCHERS.iter().enumerate() {
            if self.icon_rect(k).contains(x, y) && double {
                self.open(kind);
            }
        }
    }

    fn app_click(&mut self, i: usize, x: i32, y: i32, double: bool) {
        let mut launch: Option<Vec<Vec<u8>>> = None;
        let r = self.wins[i].r;
        match &mut self.wins[i].app {
            App::Files(f) => {
                if y < 34 {
                    if x < 70 {
                        f.dir = dirname(&f.dir).to_vec();
                        f.load();
                    }
                } else {
                    let row = ((y - 40) / ROW_H) as usize + f.top;
                    if row < f.entries.len() {
                        f.sel = row;
                        if double {
                            if let Some(p) = f.open() {
                                launch = Some(vec![b"edit".to_vec(), p]);
                            }
                        }
                    }
                }
            }
            App::Storage(_) => {
                for (k, (_, cmd)) in STORAGE_BUTTONS.iter().enumerate() {
                    if storage_button(k).contains(x, y) {
                        launch = Some(vec![b"sh".to_vec(), b"-c".to_vec(), cmd.to_vec()]);
                    }
                }
            }
            _ => {}
        }
        self.mark(r);
        if let Some(argv) = launch {
            let a: Vec<&[u8]> = argv.iter().map(|v| &v[..]).collect();
            self.open_cmd(Kind::Terminal, &a);
        }
    }

    fn keys(&mut self, bytes: &[u8]) {
        let Some(i) = self.focused() else { return };
        let r = self.wins[i].r;
        let mut launch = None;
        match &mut self.wins[i].app {
            App::Term(t) => t.send(bytes),
            App::Files(f) => {
                let n = f.entries.len();
                match bytes {
                    b"\x1b[A" => f.sel = f.sel.saturating_sub(1),
                    b"\x1b[B" => f.sel = (f.sel + 1).min(n.saturating_sub(1)),
                    b"\r" | b"\n" => {
                        if let Some(p) = f.open() {
                            launch = Some(p);
                        }
                    }
                    b"\x7f" | b"\x08" => {
                        f.dir = dirname(&f.dir).to_vec();
                        f.load();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.mark(r);
        if let Some(p) = launch {
            self.open_cmd(Kind::Terminal, &[b"edit", &p]);
        }
    }

    // ------------------------------------------------------------ drawing

    fn paint(&mut self, r: Rect) {
        self.cv.clip = r;
        let full = self.full();
        let cv = &mut self.cv;
        let f = &self.font;
        cv.gradient(full, DESK_TOP, DESK_BOT);
        // Wordmark in the corner of the wallpaper.
        let wm = b"teenyOS";
        let (wx, wy) = (full.w - wm.len() as i32 * f.w * 4 - 40, full.h - BAR_H - f.h * 4 - 30);
        text_big(cv, f, wx + 3, wy + 3, wm, 4, rgb(0x10, 0x22, 0x30));
        text_big(cv, f, wx, wy, wm, 4, rgb(0x6f, 0xc3, 0xd0));

        for (k, &(kind, name)) in LAUNCHERS.iter().enumerate() {
            let ir = Rect::new(18, 18 + k as i32 * 84, 72, 76);
            let (bmp, col) = icon_of(kind);
            cv.round(Rect::new(ir.x + 12, ir.y, 48, 48), rgb(0xe8, 0xee, 0xf6));
            cv.bitmap(ir.x + 20, ir.y + 8, bmp, 2, col, rgb(0x25, 0x2a, 0x35));
            let tx = ir.x + (ir.w - name.len() as i32 * f.w) / 2;
            cv.text(f, tx + 1, ir.y + 55, name, rgb(0, 0, 0), None);
            cv.text(f, tx, ir.y + 54, name, WHITE, None);
        }

        let top = self.wins.iter().rposition(|w| !w.min);
        for (i, w) in self.wins.iter().enumerate() {
            if w.min || w.r.intersect(&r).empty() {
                continue;
            }
            let focus = Some(i) == top;
            // Shadow, frame, title bar.
            cv.fill(Rect::new(w.r.x + 4, w.r.y + 4, w.r.w, w.r.h), rgb(0x0a, 0x14, 0x1e));
            cv.fill(w.r, WIN_EDGE);
            cv.fill(Rect::new(w.r.x + 1, w.r.y + 1, w.r.w - 2, TITLE_H - 1), if focus { ACCENT } else { TITLE_OFF });
            let max = ((w.r.w - 70) / f.w).max(0) as usize;
            cv.text(f, w.r.x + 10, w.r.y + (TITLE_H - f.h) / 2, &w.title[..w.title.len().min(max)], WHITE, None);
            let cb = w.close_box();
            cv.round(cb, rgb(0xe5, 0x48, 0x4d));
            cv.text(f, cb.x + 6, cb.y + (cb.h - f.h) / 2, b"x", WHITE, None);
            let mb = w.min_box();
            cv.round(mb, rgb(0x5f, 0x6b, 0x7d));
            cv.text(f, mb.x + 6, mb.y + (mb.h - f.h) / 2 - 2, b"_", WHITE, None);
            let c = w.content();
            let saved = cv.clip;
            cv.clip = saved.intersect(&c);
            draw_app(cv, f, &w.app, c, focus);
            cv.clip = saved;
        }

        // Taskbar.
        let bar = Rect::new(0, full.h - BAR_H, full.w, BAR_H);
        cv.fill(bar, BAR);
        cv.fill(Rect::new(0, bar.y, full.w, 1), rgb(0x3a, 0x40, 0x52));
        let sb = Rect::new(6, bar.y + 5, 100, BAR_H - 10);
        cv.round(sb, if self.menu { ACCENT } else { BAR_HI });
        cv.round(Rect::new(sb.x + 8, sb.y + 6, 12, 12), rgb(0x6f, 0xc3, 0xd0));
        cv.text(f, sb.x + 28, sb.y + (sb.h - f.h) / 2, b"teenyOS", WHITE, None);
        let mut bx = 120;
        for (i, w) in self.wins.iter().enumerate() {
            let b = Rect::new(bx, bar.y + 5, 160, BAR_H - 10);
            let col = if Some(i) == top { rgb(0x3a, 0x42, 0x58) } else { BAR_HI };
            cv.round(b, col);
            if Some(i) == top {
                cv.fill(Rect::new(b.x + 4, b.y + b.h - 2, b.w - 8, 2), ACCENT);
            }
            let t = &w.title[..w.title.len().min(18)];
            cv.text(f, b.x + 8, b.y + (b.h - f.h) / 2, t, if w.min { MUTED } else { WHITE }, None);
            bx += 166;
        }
        let right = [&self.status[..], b"   ", &self.clock[..]].concat();
        cv.text(f, full.w - right.len() as i32 * f.w - 14, bar.y + (BAR_H - f.h) / 2, &right, rgb(0xd0, 0xd6, 0xe0), None);

        if self.menu {
            let m = Rect::new(4, full.h - BAR_H - (MENU.len() as i32 * ROW_H + 40) - 4, 200, MENU.len() as i32 * ROW_H + 40);
            cv.fill(Rect::new(m.x + 4, m.y + 4, m.w, m.h), rgb(0x05, 0x08, 0x0c));
            cv.round(m, rgb(0x20, 0x24, 0x31));
            cv.text(f, m.x + 14, m.y + 10, b"teenyOS", rgb(0x6f, 0xc3, 0xd0), None);
            for (k, item) in MENU.iter().enumerate() {
                let iy = m.y + 36 + k as i32 * ROW_H;
                if item.is_empty() {
                    cv.fill(Rect::new(m.x + 10, iy + ROW_H / 2, m.w - 20, 1), rgb(0x3a, 0x40, 0x52));
                    continue;
                }
                let hover = Rect::new(m.x + 4, iy, m.w - 8, ROW_H).contains(self.mx, self.my);
                if hover {
                    cv.round(Rect::new(m.x + 4, iy, m.w - 8, ROW_H), ACCENT);
                }
                cv.text(f, m.x + 18, iy + (ROW_H - f.h) / 2, item, WHITE, None);
            }
        }

        cv.bitmap(self.mx, self.my, CURSOR, 1, WHITE, rgb(0, 0, 0));
    }

    fn render(&mut self) {
        let rects = core::mem::take(&mut self.dirty);
        for r in rects {
            self.paint(r);
            self.scr.blit(&self.cv, r);
        }
    }

    /// Once a second: clock, battery, persistence, and monitor windows.
    fn tick(&mut self) {
        let t = sys::now();
        let s = t % 86400;
        let clock = [&two(s / 3600)[..], b":", &two(s / 60 % 60)[..]].concat();
        let mut status = Vec::new();
        if let Ok(b) = read_file(b"/sys/class/power_supply/BAT0/capacity") {
            status.extend_from_slice(b"battery ");
            status.extend_from_slice(trim(&b));
            status.extend_from_slice(b"%   ");
        }
        if sys::stat(b"/sys/class/net").is_some() {
            match read_file(b"/run/net") {
                Ok(n) if !n.is_empty() => {
                    status.extend_from_slice(b"online ");
                    status.extend_from_slice(n.split(|&c| c == b' ').nth(1).unwrap_or(b""));
                }
                _ => status.extend_from_slice(b"offline"),
            }
            status.extend_from_slice(b"   ");
        }
        let p = persist_status();
        status.extend_from_slice(if p.is_empty() { b"RAM only" } else { b"saving to disk" });
        if clock != self.clock || status != self.status {
            self.clock = clock;
            self.status = status;
            self.mark(self.bar());
        }
        for i in 0..self.wins.len() {
            if matches!(self.wins[i].app, App::Monitor(..) | App::Storage(..)) && !self.wins[i].min {
                refresh(&mut self.wins[i].app);
                let c = self.wins[i].content();
                self.mark(c);
            }
        }
    }
}

const STORAGE_BUTTONS: &[(&[u8], &[u8])] = &[
    (b"Turn on persistence", b"persist enable; echo; echo 'Press Enter to close.'; read x"),
    (b"Turn off", b"persist disable; echo; echo 'Press Enter to close.'; read x"),
    (b"Install to a disk...", b"teeny-install; echo; echo 'Press Enter to close.'; read x"),
];

fn storage_button(k: usize) -> Rect {
    let x = [16, 196, 296][k];
    let w = [170, 90, 190][k];
    Rect::new(x, 190, w, 30)
}

fn refresh(app: &mut App) {
    match app {
        App::Monitor(lines) => {
            let mi = read_file(b"/proc/meminfo").unwrap_or_default();
            let get = |k: &[u8]| -> u64 {
                lines_of(&mi)
                    .find(|l| l.starts_with(k))
                    .and_then(|l| trim(&l[k.len()..]).split(|&c| c == b' ').next().and_then(parse_u64))
                    .unwrap_or(0)
            };
            let (total, avail) = (get(b"MemTotal:"), get(b"MemAvailable:"));
            let up = read_file(b"/proc/uptime").unwrap_or_default();
            let secs = up.split(|&c| c == b'.').next().and_then(parse_u64).unwrap_or(0);
            let load = read_file(b"/proc/loadavg").unwrap_or_default();
            let load: Vec<u8> = load.split(|&c| c == b' ').take(3).collect::<Vec<_>>().join(&b' ');
            let mut procs: Vec<(u64, Vec<u8>)> = Vec::new();
            for n in list_dir(b"/proc").unwrap_or_default() {
                if parse_u64(&n).is_none() {
                    continue;
                }
                let st = read_file(&[b"/proc/" as &[u8], &n, b"/stat"].concat()).unwrap_or_default();
                let (Some(o), Some(c)) = (st.iter().position(|&c| c == b'('), st.iter().rposition(|&c| c == b')')) else { continue };
                let rest: Vec<&[u8]> = st[c + 2..].split(|&c| c == b' ').collect();
                let rss = rest.get(21).and_then(|r| parse_u64(r)).unwrap_or(0) * 4;
                if rss > 0 {
                    procs.push((rss, st[o + 1..c].to_vec()));
                }
            }
            procs.sort_by(|a, b| b.0.cmp(&a.0));
            let used = total - avail;
            *lines = vec![
                [b"Memory  " as &[u8], &human(used * 1024), b" used of ", &human(total * 1024)].concat(),
                num(used * 1000 / total.max(1)),
                [b"Uptime  " as &[u8], &num(secs / 3600), b"h ", &two(secs / 60 % 60), b"m ", &two(secs % 60), b"s"].concat(),
                [b"Load    " as &[u8], &load].concat(),
                [b"Tasks   " as &[u8], &num(list_dir(b"/proc").unwrap_or_default().iter().filter(|n| parse_u64(n).is_some()).count() as u64)].concat(),
            ];
            for (rss, name) in procs.iter().take(8) {
                lines.push([&name[..], b"\t", &num(*rss), b" KB"].concat());
            }
        }
        App::Storage(lines) => {
            let p = persist_status();
            *lines = vec![
                if p.is_empty() {
                    b"Persistence is OFF: everything is in RAM and is lost at power off.".to_vec()
                } else {
                    [b"Persistence is ON: /root is saved on " as &[u8], &p].concat()
                },
                Vec::new(),
                b"Turn on: keeps /root on this USB stick (or another FAT disk)".to_vec(),
                b"in a folder called teenyos-data. Turn off: back to RAM only.".to_vec(),
                b"Install: puts teenyOS on a disk so this PC boots it directly.".to_vec(),
            ];
        }
        _ => {}
    }
}

fn lines_of(d: &[u8]) -> impl Iterator<Item = &[u8]> {
    d.split(|&c| c == b'\n')
}

fn text_big(cv: &mut Canvas, f: &Font, x: i32, y: i32, s: &[u8], scale: i32, col: u32) {
    // Render each glyph into a small canvas, then scale it up.
    let mut tmp = Canvas::new(f.w, f.h);
    for (k, &ch) in s.iter().enumerate() {
        tmp.px.fill(0);
        tmp.glyph(f, 0, 0, ch, 1, None);
        let gx = x + k as i32 * f.w * scale;
        for py in 0..f.h {
            for px in 0..f.w {
                if tmp.px[(py * f.w + px) as usize] == 1 {
                    cv.fill(Rect::new(gx + px * scale, y + py * scale, scale, scale), col);
                }
            }
        }
    }
}

fn button(cv: &mut Canvas, f: &Font, r: Rect, label: &[u8], primary: bool) {
    cv.round(r, if primary { ACCENT } else { rgb(0xdd, 0xe1, 0xe8) });
    let tx = r.x + (r.w - label.len() as i32 * f.w) / 2;
    cv.text(f, tx, r.y + (r.h - f.h) / 2, label, if primary { WHITE } else { INK }, None);
}

fn draw_app(cv: &mut Canvas, f: &Font, app: &App, c: Rect, focus: bool) {
    match app {
        App::Term(t) => t.draw(cv, f, c, focus),
        App::Files(fl) => {
            cv.fill(c, WIN_BG);
            cv.fill(Rect::new(c.x, c.y, c.w, 34), rgb(0xe6, 0xe9, 0xee));
            button(cv, f, Rect::new(c.x + 8, c.y + 5, 56, 24), b"Up", false);
            cv.text(f, c.x + 76, c.y + 9, &fl.dir, INK, None);
            let rows = ((c.h - 44) / ROW_H) as usize;
            for (k, (name, is_dir, size)) in fl.entries.iter().enumerate().skip(fl.top).take(rows) {
                let y = c.y + 40 + (k - fl.top) as i32 * ROW_H;
                if k == fl.sel {
                    cv.fill(Rect::new(c.x + 4, y, c.w - 8, ROW_H), SEL);
                }
                let col = if *is_dir { rgb(0xf6, 0xc3, 0x4b) } else { rgb(0xa8, 0xb4, 0xc8) };
                cv.round(Rect::new(c.x + 12, y + 4, 16, 14), col);
                cv.text(f, c.x + 38, y + (ROW_H - f.h) / 2, name, INK, None);
                if !is_dir {
                    let s = human(*size);
                    cv.text(f, c.x + c.w - 16 - s.len() as i32 * f.w, y + (ROW_H - f.h) / 2, &s, MUTED, None);
                }
            }
            if fl.entries.len() <= 1 {
                cv.text(f, c.x + 38, c.y + 70, b"(empty folder)", MUTED, None);
            }
        }
        App::Monitor(lines) => {
            cv.fill(c, WIN_BG);
            let mut y = c.y + 14;
            for (k, l) in lines.iter().enumerate() {
                if k == 1 {
                    let per = parse_u64(l).unwrap_or(0) as i32;
                    let bar = Rect::new(c.x + 16, y, c.w - 32, 14);
                    cv.round(bar, rgb(0xdd, 0xe1, 0xe8));
                    cv.round(Rect::new(bar.x, bar.y, (bar.w * per / 1000).max(6), bar.h), rgb(0x4a, 0xd6, 0x8e));
                    y += 26;
                    continue;
                }
                if k == 5 {
                    y += 6;
                    cv.text(f, c.x + 16, y, b"Process", MUTED, None);
                    cv.text(f, c.x + c.w - 16 - 6 * f.w, y, b"Memory", MUTED, None);
                    y += f.h + 6;
                }
                if let Some(tab) = l.iter().position(|&ch| ch == b'\t') {
                    cv.text(f, c.x + 16, y, &l[..tab], INK, None);
                    let v = &l[tab + 1..];
                    cv.text(f, c.x + c.w - 16 - v.len() as i32 * f.w, y, v, MUTED, None);
                } else {
                    cv.text(f, c.x + 16, y, l, INK, None);
                }
                y += f.h + 6;
            }
        }
        App::Storage(lines) => {
            cv.fill(c, WIN_BG);
            let mut y = c.y + 16;
            for l in lines {
                cv.text(f, c.x + 16, y, l, INK, None);
                y += f.h + 8;
            }
            for (k, (label, _)) in STORAGE_BUTTONS.iter().enumerate() {
                let b = storage_button(k);
                button(cv, f, Rect::new(c.x + b.x, c.y + b.y, b.w, b.h), label, k == 0);
            }
        }
        App::About => {
            cv.fill(c, WIN_BG);
            text_big(cv, f, c.x + 24, c.y + 20, b"teenyOS", 4, ACCENT);
            let mut u = [0u8; 390];
            sys::sc(sys::SYS_UNAME, u.as_mut_ptr() as usize, 0, 0, 0, 0);
            let rel = unsafe { crate::rt::cstr(u.as_ptr().add(130)) };
            let lines: [&[u8]; 5] = [
                &[b"Linux " as &[u8], rel, b", x86_64"].concat(),
                b"A whole desktop OS in about 2 MB.",
                b"Runs from RAM. Boots from USB on BIOS and UEFI PCs.",
                b"Kernel: Linux. Everything else: one Rust program.",
                b"",
            ];
            let mut y = c.y + 20 + f.h * 4 + 20;
            for l in lines {
                cv.text(f, c.x + 26, y, l, INK, None);
                y += f.h + 6;
            }
        }
    }
}

pub fn main(_: &[&[u8]]) -> i32 {
    let Some(scr) = Screen::open() else {
        msg(b"desktop", b"no graphical framebuffer (/dev/fb0); staying in text mode");
        return 1;
    };
    let Some(font) = Font::from_console(0) else {
        msg(b"desktop", b"could not read the console font");
        return 1;
    };
    let Some(saved) = sys::Termios::get(0) else {
        msg(b"desktop", b"must run on a console");
        return 1;
    };
    saved.raw(false).set(0);
    sys::ioctl(0, sys::KDSETMODE, sys::KD_GRAPHICS);
    let mouse = sys::open(b"/dev/input/mice", sys::O_RDONLY | sys::O_NONBLOCK, 0);

    let (w, h) = (scr.w, scr.h);
    let mut d = Desk {
        cv: Canvas::new(w, h),
        scr,
        font,
        wins: Vec::new(),
        mx: w / 2,
        my: h / 2,
        buttons: 0,
        drag: None,
        menu: false,
        dirty: Vec::new(),
        last_click: (0, 0, 0),
        clock: Vec::new(),
        status: Vec::new(),
        quit: None,
        spawn_n: 0,
    };
    d.tick();
    let full = d.full();
    d.mark(full);
    // A terminal is the most useful thing to start with.
    d.open(Kind::Terminal);
    d.render();

    let mut mbuf: Vec<u8> = Vec::new();
    let mut last_tick = sys::now_ms();
    while d.quit.is_none() {
        let mut fds = vec![sys::PollFd { fd: 0, events: sys::POLLIN, revents: 0 }, sys::PollFd { fd: mouse, events: sys::POLLIN, revents: 0 }];
        for win in &d.wins {
            if let App::Term(t) = &win.app {
                fds.push(sys::PollFd { fd: t.master, events: sys::POLLIN, revents: 0 });
            }
        }
        let wait = 1000 - (sys::now_ms() - last_tick).min(1000);
        sys::poll(&mut fds, wait as i32);

        if fds[0].revents != 0 {
            let mut k = [0u8; 256];
            let n = sys::read(0, &mut k);
            if n > 0 {
                d.keys(&k[..n as usize]);
            }
        }
        if fds[1].revents != 0 {
            let mut b = [0u8; 384];
            let n = sys::read(mouse, &mut b);
            if n > 0 {
                mbuf.extend_from_slice(&b[..n as usize]);
            }
            // PS/2 packets: buttons, dx, dy. Bit 3 of the first byte is
            // always set, which lets us resynchronise after a lost byte.
            while mbuf.len() >= 3 {
                if mbuf[0] & 8 == 0 {
                    mbuf.remove(0);
                    continue;
                }
                let (b0, b1, b2) = (mbuf[0] as i32, mbuf[1] as i32, mbuf[2] as i32);
                let dx = b1 - ((b0 << 4) & 0x100);
                let dy = b2 - ((b0 << 3) & 0x100);
                mbuf.drain(..3);
                d.mouse(dx, dy, (b0 & 7) as u8);
            }
        }
        // Terminal output; close windows whose program has exited.
        let mut i = 0;
        while i < d.wins.len() {
            let mut gone = false;
            let r = d.wins[i].content();
            if let App::Term(t) = &mut d.wins[i].app {
                let before = fds.iter().any(|p| p.fd == t.master && p.revents != 0);
                if before {
                    gone = !t.pump();
                    d.mark(r);
                }
            }
            if gone {
                d.close(i);
            } else {
                i += 1;
            }
        }
        if sys::now_ms() - last_tick >= 1000 {
            last_tick = sys::now_ms();
            d.tick();
            while sys::wait4(-1, sys::WNOHANG).0 > 0 {}
        }
        d.render();
    }

    let action = d.quit.take();
    drop(d);
    sys::ioctl(0, sys::KDSETMODE, sys::KD_TEXT);
    saved.set(0);
    crate::rt::out(b"\x1b[H\x1b[2J");
    crate::rt::flush();
    match action {
        Some(Action::Reboot) => crate::system::reboot(&[]),
        Some(Action::Poweroff) => crate::system::poweroff(&[]),
        _ => {
            // Tell init to give tty1 a shell instead of restarting us.
            let _ = write_file(b"/run/textmode", b"1");
            0
        }
    }
}
