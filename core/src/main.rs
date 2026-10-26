#![no_std]
#![no_main]

extern crate alloc;

mod fb;
mod font;
mod desktop;
mod proc;
mod rt;
mod sched;
mod shell;
mod sysb;
mod tty;
mod uefi;
mod util;
mod vfs;

use fb::Screen;
use uefi::{Handle, Status};

pub fn keymap(scan: u16, uni: u16) -> alloc::vec::Vec<u8> {
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

#[no_mangle]
pub extern "efiapi" fn efi_main(image: Handle, st: *mut u8) -> Status {
    uefi::init(st);
    uefi::set_image(image);
    let Some(gop) = uefi::open_gop() else { return 1 };
    let scr = Screen::new(&gop);

    vfs::init();
    vfs::set_now(uefi::wall_time());
    // The desktop (thread 0) needs a process context before any sys call.
    sched::init(proc::Proc::new(0));
    let _ = util::write_file(b"/etc/motd", b"teenyOS core -- a desktop OS with no kernel.\n");
    let _ = util::write_file(b"/root/readme.txt", b"This filesystem lives in RAM.\nOpen more terminals from the teeny menu.\n");

    desktop::run(scr)
}
