// Terminal channels. Each desktop terminal window has one of these: the shell
// thread writes bytes to it (parsed as VT100 into a cell grid) and reads the
// keystrokes the desktop feeds in. No pty and no kernel involved.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

pub const DEF_COLS: usize = 80;
pub const DEF_ROWS: usize = 24;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: u8,
    pub fg: u8,
    pub bg: u8,
}

const FG: u8 = 7;
const BG: u8 = 0;
const BLANK: Cell = Cell { ch: b' ', fg: FG, bg: BG };

enum St {
    Normal,
    Esc,
    Csi,
    Osc,
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cx: usize,
    pub cy: usize,
    pub cursor_on: bool,
    pub dirty: bool,
    pub alive: bool,
    input: VecDeque<u8>,
    fg: u8,
    bg: u8,
    bold: bool,
    reverse: bool,
    st: St,
    params: Vec<u8>,
    saved: (usize, usize),
}

impl Term {
    fn new(cols: usize, rows: usize) -> Term {
        Term {
            cols,
            rows,
            cells: vec![BLANK; cols * rows],
            cx: 0,
            cy: 0,
            cursor_on: true,
            dirty: true,
            alive: true,
            input: VecDeque::new(),
            fg: FG,
            bg: BG,
            bold: false,
            reverse: false,
            st: St::Normal,
            params: Vec::new(),
            saved: (0, 0),
        }
    }

    fn blank(&self) -> Cell {
        Cell { ch: b' ', fg: self.fg, bg: self.bg }
    }

    fn scroll(&mut self) {
        self.cells.drain(..self.cols);
        let b = self.blank();
        self.cells.extend(core::iter::repeat(b).take(self.cols));
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.scroll();
        } else {
            self.cy += 1;
        }
    }

    fn clear(&mut self, from: usize, to: usize) {
        let b = self.blank();
        let len = self.cells.len();
        for c in &mut self.cells[from.min(len)..to.min(len)] {
            *c = b;
        }
    }

    fn param(&self, i: usize, def: usize) -> usize {
        let s = self.params.strip_prefix(b"?").unwrap_or(&self.params);
        match s.split(|&c| c == b';').nth(i).and_then(crate::util::parse_u64) {
            Some(0) | None => def,
            Some(n) => n as usize,
        }
    }

    fn feed(&mut self, b: u8) {
        match self.st {
            St::Normal => match b {
                27 => self.st = St::Esc,
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
                    self.cells[y * self.cols + x] = Cell { ch: c, fg, bg };
                    self.cx += 1;
                }
                _ => {}
            },
            St::Esc => {
                self.st = St::Normal;
                match b {
                    b'[' => {
                        self.params.clear();
                        self.st = St::Csi;
                    }
                    b']' => self.st = St::Osc,
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
            St::Osc => {
                if b == 7 || b == 27 {
                    self.st = St::Normal;
                }
            }
            St::Csi => {
                if b.is_ascii_digit() || b == b';' || b == b'?' {
                    if self.params.len() < 32 {
                        self.params.push(b);
                    }
                    return;
                }
                self.st = St::Normal;
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
                    1 => self.clear(0, here),
                    2 => self.clear(0, cols * rows),
                    _ => self.clear(here, cols * rows),
                }
            }
            b'K' => {
                let start = self.cy * cols;
                let here = start + self.cx.min(cols);
                match self.param(0, 0) {
                    1 => self.clear(start, here),
                    2 => self.clear(start, start + cols),
                    _ => self.clear(here, start + cols),
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
                if self.param(0, 0) == 25 {
                    self.cursor_on = on;
                } else if matches!(self.param(0, 0), 1049 | 47 | 1047) {
                    self.clear(0, cols * rows);
                    self.cx = 0;
                    self.cy = 0;
                }
            }
            b's' => self.saved = (self.cx, self.cy),
            b'u' => (self.cx, self.cy) = self.saved,
            _ => {}
        }
    }
}

// Global registry of terminals, keyed by id.
struct SyncCell(RefCell<Vec<Option<Term>>>);
unsafe impl Sync for SyncCell {}
static TTYS: SyncCell = SyncCell(RefCell::new(Vec::new()));

fn with<R>(f: impl FnOnce(&mut Vec<Option<Term>>) -> R) -> R {
    f(&mut TTYS.0.borrow_mut())
}

pub fn create(cols: usize, rows: usize) -> u32 {
    with(|v| {
        v.push(Some(Term::new(cols, rows)));
        (v.len() - 1) as u32
    })
}

pub fn destroy(id: u32) {
    with(|v| {
        if let Some(slot) = v.get_mut(id as usize) {
            *slot = None;
        }
    });
}

/// Shell wrote bytes to its terminal: parse them into the grid.
pub fn write(id: u32, data: &[u8]) {
    with(|v| {
        if let Some(Some(t)) = v.get_mut(id as usize) {
            // Cooked output: a bare newline also returns the carriage, as a
            // pseudo-terminal's ONLCR would. Programs that send \r\n are
            // unaffected (a second CR is a no-op).
            for &b in data {
                if b == b'\n' {
                    t.feed(b'\r');
                }
                t.feed(b);
            }
            t.dirty = true;
        }
    });
}

/// Desktop injects keystrokes for the focused terminal.
pub fn push_input(id: u32, data: &[u8]) {
    with(|v| {
        if let Some(Some(t)) = v.get_mut(id as usize) {
            t.input.extend(data.iter().copied());
        }
    });
}

/// Shell reads a keystroke; None if nothing is queued.
pub fn read_input(id: u32) -> Option<u8> {
    with(|v| v.get_mut(id as usize).and_then(|s| s.as_mut()).and_then(|t| t.input.pop_front()))
}

/// Run `f` with the terminal's grid, for drawing. Clears the dirty flag.
pub fn render<R>(id: u32, f: impl FnOnce(&Term) -> R) -> Option<R> {
    with(|v| {
        v.get_mut(id as usize).and_then(|s| s.as_mut()).map(|t| {
            t.dirty = false;
            f(t)
        })
    })
}

pub fn is_dirty(id: u32) -> bool {
    with(|v| v.get(id as usize).and_then(|s| s.as_ref()).map_or(false, |t| t.dirty))
}

pub fn size(id: u32) -> (usize, usize) {
    with(|v| v.get(id as usize).and_then(|s| s.as_ref()).map_or((DEF_COLS, DEF_ROWS), |t| (t.cols, t.rows)))
}

const PALETTE: [u32; 16] = [
    0x1e1e2e, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xd8dce4,
    0x5c6370, 0xff868e, 0xb5e890, 0xffd88f, 0x82c8ff, 0xde9bf0, 0x7fdbe6, 0xffffff,
];

pub fn palette(i: u8) -> u32 {
    PALETTE[(i & 15) as usize]
}
