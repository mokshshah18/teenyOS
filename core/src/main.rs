#![no_std]
#![no_main]

extern crate alloc;

mod fb;
mod font;
mod proc;
mod rt;
mod sched;
mod shell;
mod sysb;
mod tty;
mod uefi;
mod util;
mod vfs;

use fb::{Canvas, Rect, Screen};
use uefi::{Handle, Status};

fn key_bytes(scan: u16, uni: u16) -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;
    let s: &[u8] = match scan {
        1 => b"\x1b[A",
        2 => b"\x1b[B",
        3 => b"\x1b[C",
        4 => b"\x1b[D",
        5 => b"\x1b[H",
        6 => b"\x1b[F",
        8 => b"\x1b[3~",
        9 => b"\x1b[5~",
        10 => b"\x1b[6~",
        0x17 => b"\x1b",
        _ => {
            if uni == 0 {
                return Vec::new();
            }
            let b = if uni == b'\r' as u16 { b'\n' } else { uni as u8 };
            return alloc::vec![b];
        }
    };
    s.to_vec()
}

fn draw_term(cv: &mut Canvas, id: u32, area: Rect) {
    cv.fill(area, tty::palette(0));
    tty::render(id, |t| {
        for y in 0..t.rows {
            let py = area.y + 2 + y as i32 * fb::FH;
            for x in 0..t.cols {
                let c = t.cells[y * t.cols + x];
                let px = area.x + 2 + x as i32 * fb::FW;
                let cbg = if c.bg == 0 { None } else { Some(tty::palette(c.bg)) };
                if c.ch != b' ' || cbg.is_some() {
                    cv.glyph(px, py, c.ch, tty::palette(c.fg), cbg);
                }
            }
        }
        if t.cursor_on {
            let x = area.x + 2 + t.cx.min(t.cols - 1) as i32 * fb::FW;
            let y = area.y + 2 + t.cy as i32 * fb::FH;
            cv.fill(Rect::new(x, y + fb::FH - 2, fb::FW, 2), tty::palette(15));
        }
    });
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image: Handle, st: *mut u8) -> Status {
    uefi::init(st);
    uefi::set_image(image);
    let Some(gop) = uefi::open_gop() else { return 1 };
    let scr = Screen::new(&gop);

    vfs::init();
    vfs::set_now(uefi::wall_time());

    let cols = ((scr.w - 8) / fb::FW).max(20) as usize;
    let rows = ((scr.h - 8) / fb::FH).max(8) as usize;
    let term = tty::create(cols, rows);

    sched::init(proc::Proc::new(term));
    let _ = util::write_file(b"/etc/motd", b"Welcome to teenyOS core.\n");
    let _ = util::write_file(b"/root/readme.txt", b"This filesystem lives in RAM.\nTurn on persistence to keep it.\n");
    sched::spawn(proc::Proc::new(term), shell::main);

    let mut cv = Canvas::new(scr.w, scr.h);
    let area = Rect::new(0, 0, scr.w, scr.h);
    let key_ev = uefi::key_event();
    let timer = uefi::create_timer();

    loop {
        sysb::tick_ms(15);
        let mut got_key = false;
        while let Some((scan, uni)) = uefi::read_key() {
            let b = key_bytes(scan, uni);
            if !b.is_empty() {
                tty::push_input(term, &b);
                got_key = true;
            }
        }
        let worked = sched::pump();
        if tty::is_dirty(term) {
            draw_term(&mut cv, term, area);
            scr.blit(&cv, area);
        }
        if !worked && !got_key {
            uefi::wait_any(&[key_ev, timer]);
        } else {
            uefi::stall(2_000);
        }
    }
}
