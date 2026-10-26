// A small full-screen text editor.
//   arrows/Home/End/PgUp/PgDn move, ^S save, ^Q quit, ^F find,
//   ^K cut line, ^U paste line, ^G go to line.

use crate::rt::{flush, out};
use crate::sys;
use crate::util::*;
use alloc::vec::Vec;

const TAB: usize = 8;

struct Ed {
    path: Vec<u8>,
    lines: Vec<Vec<u8>>,
    cy: usize,
    cx: usize,
    top: usize,
    left: usize,
    rows: usize,
    cols: usize,
    dirty: bool,
    status: Vec<u8>,
    clip: Vec<Vec<u8>>,
    quit_armed: bool,
    // Set when more than the cursor line may have changed.
    full: bool,
}

fn render_col(line: &[u8], cx: usize) -> usize {
    let mut c = 0;
    for &b in &line[..cx.min(line.len())] {
        c = if b == b'\t' {
            (c / TAB + 1) * TAB
        } else {
            c + 1
        };
    }
    c
}

fn read_key() -> u32 {
    let mut k = [0u8; 1];
    if sys::read(0, &mut k) != 1 {
        return 17; // treat EOF as ^Q
    }
    if k[0] != 27 {
        return k[0] as u32;
    }
    let mut s = [0u8; 2];
    if sys::read(0, &mut s[..1]) != 1 || sys::read(0, &mut s[1..]) != 1 {
        return 27;
    }
    let mut c = s[1];
    if c.is_ascii_digit() {
        let mut t = [0u8; 1];
        sys::read(0, &mut t);
        c = match c {
            b'1' | b'7' => b'H',
            b'4' | b'8' => b'F',
            b'3' => b'X',
            b'5' => b'P',
            b'6' => b'N',
            _ => 0,
        };
    }
    1000 + c as u32
}

impl Ed {
    fn line(&self) -> &Vec<u8> {
        &self.lines[self.cy]
    }

    fn scroll(&mut self) {
        let text_rows = self.rows - 1;
        if self.cy < self.top {
            self.top = self.cy;
        }
        if self.cy >= self.top + text_rows {
            self.top = self.cy + 1 - text_rows;
        }
        let rx = render_col(self.line(), self.cx);
        if rx < self.left {
            self.left = rx;
        }
        if rx >= self.left + self.cols {
            self.left = rx + 1 - self.cols;
        }
    }

    fn draw_row(&self, r: usize) {
        let i = self.top + r;
        out(b"\x1b[");
        out_num(r as u64 + 1);
        out(b"H");
        if i < self.lines.len() {
            let mut col = 0;
            let mut shown = 0;
            for &b in &self.lines[i] {
                let w = if b == b'\t' { TAB - col % TAB } else { 1 };
                for _ in 0..w {
                    if col >= self.left && shown < self.cols {
                        out(&[if b == b'\t' {
                            b' '
                        } else if b < 32 {
                            b'?'
                        } else {
                            b
                        }]);
                        shown += 1;
                    }
                    col += 1;
                }
            }
        } else {
            out(b"\x1b[34m~\x1b[0m");
        }
        out(b"\x1b[K");
    }

    fn draw(&mut self) {
        let (top, left) = (self.top, self.left);
        self.scroll();
        out(b"\x1b[?25l");
        // Over a serial line a full repaint is slow, so when nothing
        // scrolled and only the cursor line can have changed, repaint that.
        if self.full || top != self.top || left != self.left {
            for r in 0..self.rows - 1 {
                self.draw_row(r);
            }
        } else {
            self.draw_row(self.cy - self.top);
        }
        self.full = false;
        // Status line.
        out(b"\x1b[");
        out_num(self.rows as u64);
        out(b"H\x1b[7m");
        let mut s = Vec::new();
        s.extend_from_slice(b" ");
        s.extend_from_slice(if self.path.is_empty() {
            b"[new]"
        } else {
            &self.path
        });
        if self.dirty {
            s.extend_from_slice(b" *");
        }
        s.extend_from_slice(b"  ");
        s.extend_from_slice(&num(self.cy as u64 + 1));
        s.push(b'/');
        s.extend_from_slice(&num(self.lines.len() as u64));
        s.extend_from_slice(b"  ");
        if self.status.is_empty() {
            s.extend_from_slice(b"^S save  ^Q quit  ^F find  ^K cut  ^U paste  ^G goto");
        } else {
            s.extend_from_slice(&self.status);
        }
        s.truncate(self.cols);
        while s.len() < self.cols {
            s.push(b' ');
        }
        out(&s);
        out(b"\x1b[0m");
        let rx = render_col(self.line(), self.cx) - self.left;
        out(b"\x1b[");
        out_num((self.cy - self.top + 1) as u64);
        out(b";");
        out_num(rx as u64 + 1);
        out(b"H\x1b[?25h");
        flush();
    }

    /// Read a line of input on the status bar.
    fn ask(&mut self, q: &[u8]) -> Option<Vec<u8>> {
        let mut a = Vec::new();
        loop {
            self.status = [q, &a[..]].concat();
            self.draw();
            match read_key() {
                13 | 10 => {
                    self.status.clear();
                    return Some(a);
                }
                27 | 3 | 17 => {
                    self.status.clear();
                    return None;
                }
                8 | 127 => {
                    a.pop();
                }
                c if (32..127).contains(&c) => a.push(c as u8),
                _ => {}
            }
        }
    }

    fn save(&mut self) {
        if self.path.is_empty() {
            match self.ask(b"Save as: ") {
                Some(p) if !p.is_empty() => self.path = p,
                _ => return,
            }
        }
        let mut data = Vec::new();
        for l in &self.lines {
            data.extend_from_slice(l);
            data.push(b'\n');
        }
        match write_file(&self.path, &data) {
            Ok(()) => {
                self.dirty = false;
                self.status = [b"wrote " as &[u8], &num(data.len() as u64), b" bytes"].concat();
            }
            Err(e) => self.status = [b"save failed: " as &[u8], sys::strerror(e)].concat(),
        }
    }

    fn find(&mut self) {
        let Some(q) = self.ask(b"Find: ") else { return };
        if q.is_empty() {
            return;
        }
        let n = self.lines.len();
        for k in 0..=n {
            let i = (self.cy + k) % n;
            let from = if k == 0 {
                (self.cx + 1).min(self.lines[i].len())
            } else {
                0
            };
            if let Some(p) = find(&self.lines[i][from..], &q) {
                self.cy = i;
                self.cx = from + p;
                return;
            }
        }
        self.status = b"not found".to_vec();
    }

    fn key(&mut self, k: u32) -> bool {
        let text_rows = self.rows - 1;
        if k != 17 {
            self.quit_armed = false;
        }
        // Edits within one line and horizontal moves keep `full` false.
        let (cy, n) = (self.cy, self.lines.len());
        match k {
            17 => {
                if self.dirty && !self.quit_armed {
                    self.status = b"unsaved changes: ^Q again to quit, ^S to save".to_vec();
                    self.quit_armed = true;
                    return true;
                }
                return false;
            }
            19 => self.save(),
            6 => self.find(),
            7 => {
                if let Some(n) = self.ask(b"Line: ").and_then(|s| parse_u64(&s)) {
                    self.cy = (n.max(1) as usize - 1).min(self.lines.len() - 1);
                    self.cx = 0;
                }
            }
            11 => {
                let l = if self.lines.len() > 1 {
                    self.lines.remove(self.cy)
                } else {
                    core::mem::take(&mut self.lines[0])
                };
                self.clip = alloc::vec![l];
                self.cy = self.cy.min(self.lines.len() - 1);
                self.cx = 0;
                self.dirty = true;
            }
            21 => {
                for l in self.clip.clone() {
                    self.lines.insert(self.cy, l);
                    self.cy += 1;
                }
                self.cx = 0;
                self.dirty = true;
            }
            1065 => self.cy = self.cy.saturating_sub(1),
            1066 => self.cy = (self.cy + 1).min(self.lines.len() - 1),
            1067 => {
                if self.cx < self.line().len() {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            1068 => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.line().len();
                }
            }
            1072 | 1 => self.cx = 0,
            1070 | 5 => self.cx = self.line().len(),
            1080 => self.cy = self.cy.saturating_sub(text_rows),
            1078 => self.cy = (self.cy + text_rows).min(self.lines.len() - 1),
            13 | 10 => {
                let rest = self.lines[self.cy].split_off(self.cx);
                self.lines.insert(self.cy + 1, rest);
                self.cy += 1;
                self.cx = 0;
                self.dirty = true;
            }
            8 | 127 => {
                if self.cx > 0 {
                    self.cx -= 1;
                    let cx = self.cx;
                    self.lines[self.cy].remove(cx);
                    self.dirty = true;
                } else if self.cy > 0 {
                    let l = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.line().len();
                    self.lines[self.cy].extend_from_slice(&l);
                    self.dirty = true;
                }
            }
            1088 => {
                let cx = self.cx;
                if cx < self.line().len() {
                    self.lines[self.cy].remove(cx);
                    self.dirty = true;
                } else if self.cy + 1 < self.lines.len() {
                    let l = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].extend_from_slice(&l);
                    self.dirty = true;
                }
            }
            c if c == 9 || (32..256).contains(&c) => {
                let cx = self.cx;
                self.lines[self.cy].insert(cx, c as u8);
                self.cx += 1;
                self.dirty = true;
            }
            _ => {}
        }
        self.cx = self.cx.min(self.line().len());
        // Moving to another line repaints the old one too, which is cheap
        // enough to do as a full repaint.
        if self.cy != cy || self.lines.len() != n {
            self.full = true;
        }
        true
    }
}

pub fn main(args: &[&[u8]]) -> i32 {
    let path = args.get(1).map_or(Vec::new(), |p| p.to_vec());
    let mut lines: Vec<Vec<u8>> = match read_file(&path) {
        Ok(d) => crate::util::lines(&d).iter().map(|l| l.to_vec()).collect(),
        Err(_) => Vec::new(),
    };
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    let Some(saved) = sys::Termios::get(0) else {
        msg(b"edit", b"needs a terminal");
        return 1;
    };
    let (rows, cols) = sys::winsize(1);
    let mut ed = Ed {
        path,
        lines,
        cy: 0,
        cx: 0,
        top: 0,
        left: 0,
        rows: rows.max(3),
        cols: cols.max(20),
        dirty: false,
        status: Vec::new(),
        clip: Vec::new(),
        quit_armed: false,
        full: true,
    };
    saved.raw(false).set(0);
    out(b"\x1b[?1049h");
    loop {
        ed.draw();
        let k = read_key();
        if !ed.key(k) {
            break;
        }
    }
    out(b"\x1b[2J\x1b[H\x1b[?1049l");
    flush();
    saved.set(0);
    0
}
