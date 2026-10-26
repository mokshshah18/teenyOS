#![no_std]
#![no_main]
extern crate alloc;
mod fb; mod font; mod rt; mod uefi;
use fb::{rgb, Canvas, Rect, Screen};
use uefi::{Handle, Status};
#[no_mangle]
pub extern "efiapi" fn efi_main(image: Handle, st: *mut u8) -> Status {
    uefi::init(st); uefi::set_image(image);
    let Some(gop) = uefi::open_gop() else { return 1 };
    let scr = Screen::new(&gop);
    let mut cv = Canvas::new(scr.w, scr.h);
    cv.gradient(Rect::new(0, 0, scr.w, scr.h), rgb(0x24, 0x3b, 0x55), rgb(0x14, 0x6c, 0x7a));
    let msg = b"teenyOS core";
    cv.text_big(scr.w / 2 - msg.len() as i32 * fb::FW * 4 / 2, scr.h / 2 - 80, msg, 4, rgb(0x6f, 0xc3, 0xd0));
    let sub = b"a UEFI-native desktop -- the firmware is the driver";
    cv.text(scr.w / 2 - sub.len() as i32 * fb::FW / 2, scr.h / 2 - 4, sub, rgb(0xff, 0xff, 0xff), None);
    let sub2 = b"no kernel, no drivers shipped";
    cv.text(scr.w / 2 - sub2.len() as i32 * fb::FW / 2, scr.h / 2 + 24, sub2, rgb(0xb0, 0xc4, 0xd4), None);
    scr.blit(&cv, Rect::new(0, 0, scr.w, scr.h));
    loop { uefi::stall(1_000_000); }
}
