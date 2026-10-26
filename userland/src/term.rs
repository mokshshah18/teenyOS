// Terminal emulator for desktop windows: a shell on a pseudo-terminal and
// the subset of VT100/Linux console escapes that our programs emit.

use crate::fb::{rgb, Canvas, Font, Rect};
use crate::sys;
use alloc::vec;
use alloc::vec::Vec;

pub const PALETTE: [u32; 16] = [
    rgb(0x1e, 0x1e, 0x2e),
    rgb(0xe0, 0x6c, 0x75),
    rgb(0x98, 0xc3, 0x79),
    rgb(0xe5, 0xc0, 0x7b),
    rgb(0x61, 0xaf, 0xef),
    rgb(0xc6, 0x78, 0xdd),
    rgb(0x56, 0xb6, 0xc2),
    rgb(0xd8, 0xdc, 0xe4),
    rgb(0x5c, 0x63, 0x70),
    rgb(0xff, 0x86, 0x8e),
    rgb(0xb5, 0xe8, 0x90),
    rgb(0xff, 0xd8, 0x8f),
    rgb(0x82, 0xc8, 0xff),
    rgb(0xde, 0x9b, 0xf0),
    rgb(0x7f, 0xdb, 0xe6),
    rgb(0xff, 0xff, 0xff),
];
const FG: u8 = 7;
const BG: u8 = 0;

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: u8,
    bg: u8,
}

const BLANK: Cell = Cell { ch: b' ', fg: FG, bg: BG };

enum State {
    Normal,
    Esc,
    Csi,
    Osc,
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    cx: usize,
    cy: usize,
    fg: u8,
    bg: u8,
    bold: bool,
    reverse: bool,
    cursor_on: bool,
    state: State,
    params: Vec<u8>,
    saved: (usize, usize),
    pub master: i32,
    pub pid: isize,
}

impl Term {
    /// Start `argv` (a login shell when empty) on a new pseudo-terminal.
    pub fn spawn(cols: usize, rows: usize, argv: &[&[u8]]) -> Option<Term> {
        let (master, slave) = sys::openpty()?;
        sys::set_winsize(master, rows, cols);
        crate::rt::flush();
        let pid = sys::fork();
        if pid == 0 {
            sys::close(master);
            sys::setsid();
            let fd = sys::open_inherit(&slave, sys::O_RDWR, 0);
            if fd < 0 {
                sys::exit(1);
            }
            sys::ioctl(fd, sys::TIOCSCTTY, 0);
            for i in 0..3 {
                sys::dup2(fd, i);
            }
            if fd > 2 {
                sys::close(fd);
            }
            for s in [sys::SIGINT, sys::SIGQUIT] {
                sys::signal(s, sys::SIG_DFL);
            }
            let login: [&[u8]; 1] = [b"-sh"];
            crate::init::exec(if argv.is_empty() { &login } else { argv });
            sys::exit(127);
        }
        if pid < 0 {
            sys::close(master);
            return None;
        }
        sys::set_nonblock(master);
        Some(Term {
            cols,
            rows,
            cells: vec![BLANK; cols * rows],
            cx: 0,
            cy: 0,
            fg: FG,
            bg: BG,
            bold: false,
            reverse: false,
            cursor_on: true,
            state: State::Normal,
            params: Vec::new(),
            saved: (0, 0),
            master,
            pid,
        })
    }

    pub fn send(&self, bytes: &[u8]) {
        sys::write_all(self.master, bytes);
    }

    /// Read whatever the program wrote. Returns false once it has exited.
    pub fn pump(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        loop {
            let n = sys::read(self.master, &mut buf);
            if n > 0 {
                for &b in &buf[..n as usize] {
                    self.feed(b);
                }
                continue;
            }
            // EAGAIN: nothing more for now. EIO: the other side closed.
            return n == -11;
        }
    }

    fn cell(&mut self, x: usize, y: usize) -> &mut Cell {
        &mut self.cells[y * self.cols + x]
    }

    fn blank(&self) -> Cell {
        Cell { ch: b' ', fg: self.fg, bg: self.bg }
    }

    fn scroll_up(&mut self) {
        self.cells.drain(..self.cols);
        let b = self.blank();
        self.cells.extend(core::iter::repeat(b).take(self.cols));
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cy += 1;
        }
    }

    fn param(&self, i: usize, default: usize) -> usize {
        let s = self.params.strip_prefix(b"?").unwrap_or(&self.params);
        match s.split(|&c| c == b';').nth(i).and_then(crate::util::parse_u64) {
            Some(0) | None => default,
            Some(n) => n as usize,
        }
    }

    fn clear(&mut self, from: usize, to: usize) {
        let b = self.blank();
        for c in &mut self.cells[from..to] {
            *c = b;
        }
    }

    fn feed(&mut self, b: u8) {
        match self.state {
            State::Normal => match b {
                27 => self.state = State::Esc,
                b'\r' => self.cx = 0,
                b'\n' | 11 | 12 => self.newline(),
                8 => self.cx = self.cx.saturating_sub(1),
                b'\t' => self.cx = ((self.cx / 8 + 1) * 8).min(self.cols - 1),
                7 => {}
                c if c >= 32 => {
                    if self.cx >= self.cols {
                        self.cx = 0;
                        self.newline();
                    }
                    let (mut fg, mut bg) = (self.fg, self.bg);
                    if self.bold && fg < 8 {
                        fg += 8;
                    }
                    if self.reverse {
                        core::mem::swap(&mut fg, &mut bg);
                    }
                    let (x, y) = (self.cx, self.cy);
                    *self.cell(x, y) = Cell { ch: c, fg, bg };
                    self.cx += 1;
                }
                _ => {}
            },
            State::Esc => {
                self.state = State::Normal;
                match b {
                    b'[' => {
                        self.params.clear();
                        self.state = State::Csi;
                    }
                    b']' => self.state = State::Osc,
                    b'c' => {
                        self.fg = FG;
                        self.bg = BG;
                        self.clear(0, self.cells.len());
                        self.cx = 0;
                        self.cy = 0;
                    }
                    b'7' => self.saved = (self.cx, self.cy),
                    b'8' => (self.cx, self.cy) = self.saved,
                    _ => {}
                }
            }
            State::Osc => {
                if b == 7 || b == 27 {
                    self.state = State::Normal;
                }
            }
            State::Csi => {
                if b.is_ascii_digit() || b == b';' || b == b'?' {
                    if self.params.len() < 32 {
                        self.params.push(b);
                    }
                    return;
                }
                self.state = State::Normal;
                self.csi(b);
            }
        }
    }

    fn csi(&mut self, cmd: u8) {
        let private = self.params.first() == Some(&b'?');
        let (cols, rows) = (self.cols, self.rows);
        match cmd {
            b'A' => self.cy = self.cy.saturating_sub(self.param(0, 1)),
            b'B' => self.cy = (self.cy + self.param(0, 1)).min(rows - 1),
            b'C' => self.cx = (self.cx + self.param(0, 1)).min(cols - 1),
            b'D' => self.cx = self.cx.saturating_sub(self.param(0, 1)),
            b'G' => self.cx = (self.param(0, 1) - 1).min(cols - 1),
            b'd' => self.cy = (self.param(0, 1) - 1).min(rows - 1),
            b'H' | b'f' => {
                self.cy = (self.param(0, 1) - 1).min(rows - 1);
                self.cx = (self.param(1, 1) - 1).min(cols - 1);
            }
            b'J' => {
                let here = self.cy * cols + self.cx.min(cols);
                match self.param(0, 0) {
                    0 => self.clear(here, cols * rows),
                    1 => self.clear(0, here),
                    _ => self.clear(0, cols * rows),
                }
            }
            b'K' => {
                let start = self.cy * cols;
                let here = start + self.cx.min(cols);
                match self.param(0, 0) {
                    0 => self.clear(here, start + cols),
                    1 => self.clear(start, here),
                    _ => self.clear(start, start + cols),
                }
            }
            b'm' => {
                let n = self.params.split(|&c| c == b';').count();
                for i in 0..n {
                    match self.param(i, 0) {
                        0 => {
                            self.fg = FG;
                            self.bg = BG;
                            self.bold = false;
                            self.reverse = false;
                        }
                        1 => self.bold = true,
                        7 => self.reverse = true,
                        22 => self.bold = false,
                        27 => self.reverse = false,
                        p @ 30..=37 => self.fg = (p - 30) as u8,
                        39 => self.fg = FG,
                        p @ 40..=47 => self.bg = (p - 40) as u8,
                        49 => self.bg = BG,
                        p @ 90..=97 => self.fg = (p - 82) as u8,
                        _ => {}
                    }
                }
            }
            b'h' | b'l' if private => {
                let on = cmd == b'h';
                match self.param(0, 0) {
                    25 => self.cursor_on = on,
                    1049 | 47 => {
                        self.clear(0, cols * rows);
                        self.cx = 0;
                        self.cy = 0;
                    }
                    _ => {}
                }
            }
            b's' => self.saved = (self.cx, self.cy),
            b'u' => (self.cx, self.cy) = self.saved,
            _ => {}
        }
    }

    pub fn draw(&self, cv: &mut Canvas, f: &Font, area: Rect, focused: bool) {
        cv.fill(area, PALETTE[BG as usize]);
        for y in 0..self.rows {
            let py = area.y + 4 + y as i32 * f.h;
            if py > cv.clip.y + cv.clip.h || py + f.h < cv.clip.y {
                continue;
            }
            for x in 0..self.cols {
                let c = self.cells[y * self.cols + x];
                let px = area.x + 4 + x as i32 * f.w;
                let bg = if c.bg == BG { None } else { Some(PALETTE[c.bg as usize & 15]) };
                if c.ch != b' ' || bg.is_some() {
                    cv.glyph(f, px, py, c.ch, PALETTE[c.fg as usize & 15], bg);
                }
            }
        }
        if self.cursor_on && focused {
            let x = area.x + 4 + self.cx.min(self.cols - 1) as i32 * f.w;
            let y = area.y + 4 + self.cy as i32 * f.h;
            cv.fill(Rect::new(x, y + f.h - 3, f.w, 3), PALETTE[15]);
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        sys::close(self.master);
        sys::kill(self.pid, sys::SIGHUP);
        // Give it a moment to exit, then make sure.
        for _ in 0..20 {
            if sys::wait4(self.pid, sys::WNOHANG).0 != 0 {
                return;
            }
            sys::sleep_ms(10);
        }
        sys::kill(self.pid, sys::SIGKILL);
        sys::wait4(self.pid, 0);
    }
}
