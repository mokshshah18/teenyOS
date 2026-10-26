// Raw UEFI firmware bindings and thin safe wrappers.
//
// teenyOS core never calls ExitBootServices: it runs as a UEFI application on
// top of the firmware the whole time, the way the UEFI shell and boot
// managers do. That means graphics, disk, keyboard, mouse and the network
// stack are all the firmware's code, so we ship none of it.

#![allow(dead_code)]

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

pub type Handle = *mut c_void;
pub type Status = usize;

pub const OK: Status = 0;
pub const NOT_READY: Status = 0x8000_0000_0000_0000 | 6;
pub const BUFFER_TOO_SMALL: Status = 0x8000_0000_0000_0000 | 5;
pub const NOT_FOUND: Status = 0x8000_0000_0000_0000 | 14;
pub const TIMEOUT: Status = 0x8000_0000_0000_0000 | 18;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub const GOP_GUID: Guid = Guid(0x9042a9de, 0x23dc, 0x4a38, [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a]);
pub const SIMPLE_POINTER_GUID: Guid = Guid(0x31878c87, 0x0b75, 0x11d5, [0x9a, 0x4f, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);
pub const ABSOLUTE_POINTER_GUID: Guid = Guid(0x8d59d32b, 0xc655, 0x4ae9, [0x9b, 0x15, 0xf2, 0x59, 0x04, 0x99, 0x2a, 0x43]);
pub const SFS_GUID: Guid = Guid(0x964e5b22, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const LOADED_IMAGE_GUID: Guid = Guid(0x5b1b31a1, 0x9562, 0x11d2, [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const DEVICE_PATH_GUID: Guid = Guid(0x09576e91, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
pub const FILE_INFO_GUID: Guid = Guid(0x09576e92, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);

// --- table layouts, addressed by byte offset to avoid huge repr(C) structs ---

static ST: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

pub fn init(st: *mut u8) {
    ST.store(st, Ordering::SeqCst);
}

fn st() -> *mut u8 {
    ST.load(Ordering::SeqCst)
}

unsafe fn rd<T: Copy>(base: *mut u8, off: usize) -> T {
    ptr::read_unaligned(base.add(off) as *const T)
}

fn bs() -> *mut u8 {
    unsafe { rd::<u64>(st(), 96) as *mut u8 }
}

fn rt() -> *mut u8 {
    unsafe { rd::<u64>(st(), 88) as *mut u8 }
}

fn conin() -> *mut u8 {
    unsafe { rd::<u64>(st(), 48) as *mut u8 }
}

fn conout() -> *mut u8 {
    unsafe { rd::<u64>(st(), 64) as *mut u8 }
}

// Boot-services function pointers by table index (after the 24-byte header).
unsafe fn bsfn(i: usize) -> usize {
    rd::<usize>(bs(), 24 + i * 8)
}

// --- memory pool: this is what the global allocator is built on ---

const LOADER_DATA: u32 = 2;

pub fn alloc(size: usize) -> *mut u8 {
    unsafe {
        let f: extern "efiapi" fn(u32, usize, *mut *mut u8) -> Status = core::mem::transmute(bsfn(5));
        let mut p: *mut u8 = ptr::null_mut();
        if f(LOADER_DATA, size, &mut p) == OK {
            p
        } else {
            ptr::null_mut()
        }
    }
}

pub fn free(p: *mut u8) {
    unsafe {
        let f: extern "efiapi" fn(*mut u8) -> Status = core::mem::transmute(bsfn(6));
        f(p);
    }
}

pub fn stall(micros: usize) {
    unsafe {
        let f: extern "efiapi" fn(usize) -> Status = core::mem::transmute(bsfn(28));
        f(micros);
    }
}

pub fn locate_protocol(g: &Guid) -> Handle {
    unsafe {
        let f: extern "efiapi" fn(*const Guid, *mut c_void, *mut Handle) -> Status = core::mem::transmute(bsfn(37));
        let mut p: Handle = ptr::null_mut();
        if f(g, ptr::null_mut(), &mut p) == OK {
            p
        } else {
            ptr::null_mut()
        }
    }
}

/// All handles that support a protocol. Returns a firmware-allocated buffer
/// the caller frees with `free`, and the count.
pub fn locate_handles(g: &Guid) -> (*mut Handle, usize) {
    unsafe {
        let f: extern "efiapi" fn(u32, *const Guid, *mut c_void, *mut usize, *mut *mut Handle) -> Status = core::mem::transmute(bsfn(36));
        let mut n = 0usize;
        let mut hs: *mut Handle = ptr::null_mut();
        if f(2, g, ptr::null_mut(), &mut n, &mut hs) == OK {
            (hs, n)
        } else {
            (ptr::null_mut(), 0)
        }
    }
}

pub fn handle_protocol(h: Handle, g: &Guid) -> *mut u8 {
    unsafe {
        let f: extern "efiapi" fn(Handle, *const Guid, *mut *mut u8) -> Status = core::mem::transmute(bsfn(16));
        let mut p: *mut u8 = ptr::null_mut();
        f(h, g, &mut p);
        p
    }
}

/// Connect every driver to every device, so disks and NICs come up.
pub fn connect_all() {
    unsafe {
        let (hs, n) = {
            let f: extern "efiapi" fn(u32, *const Guid, *mut c_void, *mut usize, *mut *mut Handle) -> Status = core::mem::transmute(bsfn(36));
            let mut n = 0usize;
            let mut hs: *mut Handle = ptr::null_mut();
            f(0, ptr::null(), ptr::null_mut(), &mut n, &mut hs);
            (hs, n)
        };
        if hs.is_null() {
            return;
        }
        let connect: extern "efiapi" fn(Handle, *mut Handle, *mut c_void, u8) -> Status = core::mem::transmute(bsfn(30));
        for i in 0..n {
            connect(*hs.add(i), ptr::null_mut(), ptr::null_mut(), 1);
        }
        free(hs as *mut u8);
    }
}

// --- events: used to idle without busy-spinning, and for a 1s tick ---

pub fn create_timer() -> Handle {
    unsafe {
        let f: extern "efiapi" fn(u32, usize, *mut c_void, *mut c_void, *mut Handle) -> Status = core::mem::transmute(bsfn(7));
        let mut e: Handle = ptr::null_mut();
        // EVT_TIMER (0x8000_0000), TPL_CALLBACK (8)
        if f(0x8000_0000, 8, ptr::null_mut(), ptr::null_mut(), &mut e) == OK {
            let set: extern "efiapi" fn(Handle, u32, u64) -> Status = core::mem::transmute(bsfn(8));
            set(e, 1, 10_000_000); // periodic, 1s (100ns units)
            e
        } else {
            ptr::null_mut()
        }
    }
}

/// Non-blocking: is this event signalled?
pub fn check_event(e: Handle) -> bool {
    unsafe {
        if e.is_null() {
            return false;
        }
        let f: extern "efiapi" fn(Handle) -> Status = core::mem::transmute(bsfn(12));
        f(e) == OK
    }
}

/// Block until one of the events fires. `events` must be non-empty.
pub fn wait_any(events: &[Handle]) {
    unsafe {
        let f: extern "efiapi" fn(usize, *const Handle, *mut usize) -> Status = core::mem::transmute(bsfn(9));
        let mut idx = 0usize;
        f(events.len(), events.as_ptr(), &mut idx);
    }
}

// --- text console (used only until graphics come up) ---

pub fn con_print(s: &str) {
    unsafe {
        let out = conout();
        if out.is_null() {
            return;
        }
        let f: extern "efiapi" fn(*mut u8, *const u16) -> Status = core::mem::transmute(rd::<usize>(out, 8));
        let mut buf = [0u16; 128];
        let mut n = 0;
        for c in s.bytes() {
            if n >= buf.len() - 2 {
                buf[n] = 0;
                f(out, buf.as_ptr());
                n = 0;
            }
            if c == b'\n' {
                buf[n] = 13;
                n += 1;
            }
            buf[n] = c as u16;
            n += 1;
        }
        buf[n] = 0;
        f(out, buf.as_ptr());
    }
}

// --- keyboard (Simple Text Input) ---

/// (scancode, unicode char) or None. Non-blocking.
pub fn read_key() -> Option<(u16, u16)> {
    unsafe {
        let inp = conin();
        if inp.is_null() {
            return None;
        }
        let f: extern "efiapi" fn(*mut u8, *mut [u16; 2]) -> Status = core::mem::transmute(rd::<usize>(inp, 8));
        let mut key = [0u16; 2];
        if f(inp, &mut key) == OK {
            Some((key[0], key[1]))
        } else {
            None
        }
    }
}

pub fn key_event() -> Handle {
    unsafe { rd::<usize>(conin(), 16) as Handle } // WaitForKey
}

// --- pointer: Absolute (touch/tablet) and Simple (relative mouse) ---

pub struct Pointer {
    abs: *mut u8,
    simple: *mut u8,
    pub ax_min: u64,
    pub ax_max: u64,
    pub ay_min: u64,
    pub ay_max: u64,
}

pub fn open_pointer() -> Pointer {
    let abs = locate_protocol(&ABSOLUTE_POINTER_GUID) as *mut u8;
    let simple = locate_protocol(&SIMPLE_POINTER_GUID) as *mut u8;
    let mut p = Pointer { abs, simple, ax_min: 0, ax_max: 0, ay_min: 0, ay_max: 0 };
    unsafe {
        if !abs.is_null() {
            let reset: extern "efiapi" fn(*mut u8, u8) -> Status = core::mem::transmute(rd::<usize>(abs, 0));
            reset(abs, 0);
            let mode = rd::<u64>(abs, 24) as *mut u8; // EFI_ABSOLUTE_POINTER_MODE*
            if !mode.is_null() {
                p.ax_min = rd::<u64>(mode, 0);
                p.ay_min = rd::<u64>(mode, 8);
                p.ax_max = rd::<u64>(mode, 24);
                p.ay_max = rd::<u64>(mode, 32);
            }
        }
        if !simple.is_null() {
            let reset: extern "efiapi" fn(*mut u8, u8) -> Status = core::mem::transmute(rd::<usize>(simple, 0));
            reset(simple, 0);
        }
    }
    p
}

pub enum Move {
    /// Absolute position as a fraction of the screen (0..=1 in each axis).
    Abs(f32, f32, bool),
    /// Relative movement in device units, and button state.
    Rel(i64, i64, bool),
}

impl Pointer {
    pub fn poll(&self) -> Option<Move> {
        unsafe {
            if !self.abs.is_null() && self.ax_max > self.ax_min {
                let f: extern "efiapi" fn(*mut u8, *mut u8) -> Status = core::mem::transmute(rd::<usize>(self.abs, 8));
                let mut s = [0u8; 32]; // CurrentX/Y/Z u64 + ActiveButtons u32
                if f(self.abs, s.as_mut_ptr()) == OK {
                    let x = u64::from_le_bytes(s[0..8].try_into().unwrap());
                    let y = u64::from_le_bytes(s[8..16].try_into().unwrap());
                    let btn = u32::from_le_bytes(s[24..28].try_into().unwrap()) & 1 != 0;
                    let fx = (x - self.ax_min) as f32 / (self.ax_max - self.ax_min) as f32;
                    let fy = (y - self.ay_min) as f32 / (self.ay_max - self.ay_min) as f32;
                    return Some(Move::Abs(fx, fy, btn));
                }
            }
            if !self.simple.is_null() {
                let f: extern "efiapi" fn(*mut u8, *mut u8) -> Status = core::mem::transmute(rd::<usize>(self.simple, 8));
                let mut s = [0u8; 26]; // RelX/Y/Z i64 + Left u8 + Right u8
                if f(self.simple, s.as_mut_ptr()) == OK {
                    let dx = i64::from_le_bytes(s[0..8].try_into().unwrap());
                    let dy = i64::from_le_bytes(s[8..16].try_into().unwrap());
                    return Some(Move::Rel(dx, dy, s[24] != 0));
                }
            }
        }
        None
    }

    pub fn events(&self) -> ([Handle; 2], usize) {
        let mut e = [ptr::null_mut(); 2];
        let mut n = 0;
        unsafe {
            if !self.abs.is_null() {
                e[n] = rd::<usize>(self.abs, 16) as Handle;
                n += 1;
            }
            if !self.simple.is_null() {
                e[n] = rd::<usize>(self.simple, 16) as Handle;
                n += 1;
            }
        }
        (e, n)
    }
}

// --- graphics output ---

pub struct Gop {
    pub w: i32,
    pub h: i32,
    pub handle: *mut u8,
}

pub fn open_gop() -> Option<Gop> {
    let g = locate_protocol(&GOP_GUID) as *mut u8;
    if g.is_null() {
        return None;
    }
    unsafe {
        let mode = rd::<u64>(g, 24) as *mut u8;
        if mode.is_null() {
            return None;
        }
        let info = rd::<u64>(mode, 8) as *mut u8;
        let w = rd::<u32>(info, 4) as i32;
        let h = rd::<u32>(info, 8) as i32;
        if w <= 0 || h <= 0 {
            return None;
        }
        PANIC_GOP.store(g, Ordering::SeqCst);
        Some(Gop { w, h, handle: g })
    }
}

/// Fill a rectangle with one colour, via the firmware (EfiBltVideoFill). This
/// is the path that reliably reaches the display across firmware and QEMU.
pub fn gop_fill(g: *mut u8, color: u32, x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    unsafe {
        let blt: extern "efiapi" fn(*mut u8, *const u32, usize, usize, usize, usize, usize, usize, usize, usize) -> Status =
            core::mem::transmute(rd::<usize>(g, 16));
        let c = color;
        blt(g, &c, 0, 0, 0, x as usize, y as usize, w as usize, h as usize, 0);
    }
}

// --- runtime: wall clock and reset ---

/// Seconds since 1970 (UTC) from the firmware real-time clock.
pub fn wall_time() -> u64 {
    unsafe {
        let f: extern "efiapi" fn(*mut u8, *mut u8) -> Status = core::mem::transmute(rd::<usize>(rt(), 24));
        let mut t = [0u8; 16]; // EFI_TIME
        if f(t.as_mut_ptr(), ptr::null_mut()) != OK {
            return 0;
        }
        let year = u16::from_le_bytes([t[0], t[1]]) as i64;
        let (mon, day, hh, mm, ss) = (t[2] as i64, t[3] as i64, t[4] as i64, t[5] as i64, t[6] as i64);
        days_from_civil(year, mon, day) as u64 * 86400 + (hh * 3600 + mm * 60 + ss) as u64
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn reset(shutdown: bool) -> ! {
    unsafe {
        let f: extern "efiapi" fn(u32, Status, usize, *const c_void) -> ! = core::mem::transmute(rd::<usize>(rt(), 40));
        f(if shutdown { 2 } else { 0 }, OK, 0, ptr::null()); // 2=Shutdown, 0=Cold reset
    }
}

pub fn image_handle_device() -> Handle {
    // The handle teenyOS was loaded from, so we can find the volume we booted.
    LOADED.load(Ordering::SeqCst)
}

static LOADED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static PANIC_GOP: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

/// Fill the whole screen one colour, for the panic handler to signal visibly.
pub fn mark(color: u32, x: i32) {
    let g = PANIC_GOP.load(Ordering::SeqCst);
    if !g.is_null() {
        gop_fill(g, color, x, 200, 30, 30);
    }
}

pub fn panic_flash(color: u32) {
    let g = PANIC_GOP.load(Ordering::SeqCst);
    if !g.is_null() {
        gop_fill(g, color, 0, 0, 4096, 2160);
    }
}

pub fn set_image(h: Handle) {
    LOADED.store(h, Ordering::SeqCst);
}

/// The SimpleFileSystem handle of the volume teenyOS booted from, if any.
pub fn boot_volume() -> *mut u8 {
    let img = image_handle_device();
    if img.is_null() {
        return locate_protocol(&SFS_GUID) as *mut u8;
    }
    let li = handle_protocol(img, &LOADED_IMAGE_GUID);
    if li.is_null() {
        return locate_protocol(&SFS_GUID) as *mut u8;
    }
    unsafe {
        let dev = rd::<Handle>(li, 24); // EFI_LOADED_IMAGE.DeviceHandle
        let sfs = handle_protocol(dev, &SFS_GUID);
        if sfs.is_null() {
            locate_protocol(&SFS_GUID) as *mut u8
        } else {
            sfs
        }
    }
}

// --- direct COM1 serial output, for reliable ordered debugging under QEMU ---

#[inline]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}

pub fn serial_init() {
    unsafe {
        outb(0x3f8 + 1, 0x00);
        outb(0x3f8 + 3, 0x80);
        outb(0x3f8 + 0, 0x01);
        outb(0x3f8 + 1, 0x00);
        outb(0x3f8 + 3, 0x03);
        outb(0x3f8 + 2, 0xc7);
        outb(0x3f8 + 4, 0x0b);
    }
}

pub fn serial_print(s: &str) {
    unsafe {
        for &b in s.as_bytes() {
            if b == b'\n' {
                while inb(0x3f8 + 5) & 0x20 == 0 {}
                outb(0x3f8, b'\r');
            }
            while inb(0x3f8 + 5) & 0x20 == 0 {}
            outb(0x3f8, b);
        }
    }
}

// --- networking protocol GUIDs and generic event helpers ---

pub const HTTP_SB_GUID: Guid = Guid(0xbdc8e6af, 0xd9bc, 0x4379, [0xa7, 0x2a, 0xe0, 0xc4, 0xe7, 0x5d, 0xae, 0x1c]);
pub const HTTP_GUID: Guid = Guid(0x7a59b29b, 0x910b, 0x4171, [0x82, 0x42, 0xa8, 0x5a, 0x0d, 0xf2, 0x5b, 0x5b]);
pub const DNS4_SB_GUID: Guid = Guid(0xb625b186, 0xe063, 0x44f7, [0x89, 0x05, 0x6a, 0x74, 0xdc, 0x6f, 0x52, 0xb4]);
pub const DHCP4_SB_GUID: Guid = Guid(0x9d9a39d8, 0xbd42, 0x4a73, [0xa4, 0xd5, 0x8e, 0xe9, 0x4b, 0xe1, 0x13, 0x80]);
pub const TLS_SB_GUID: Guid = Guid(0x952cb795, 0xff36, 0x48cf, [0xa2, 0x49, 0x4d, 0xf4, 0x86, 0xd6, 0xab, 0x8d]);
pub const IP4CONFIG2_GUID: Guid = Guid(0x5b446ed1, 0xe30b, 0x4faa, [0x87, 0x1a, 0x36, 0x54, 0xec, 0xa3, 0x60, 0x80]);
pub const SNP_GUID: Guid = Guid(0xa19832b9, 0xac25, 0x11d3, [0x9a, 0x2d, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);

/// A plain waitable event (no notify function), for completion tokens.
pub fn create_event() -> Handle {
    unsafe {
        let f: extern "efiapi" fn(u32, usize, *const c_void, *const c_void, *mut Handle) -> Status = core::mem::transmute(bsfn(7));
        let mut e: Handle = ptr::null_mut();
        if f(0, 8, ptr::null(), ptr::null(), &mut e) == OK {
            e
        } else {
            ptr::null_mut()
        }
    }
}

pub fn close_event(e: Handle) {
    unsafe {
        let f: extern "efiapi" fn(Handle) -> Status = core::mem::transmute(bsfn(11));
        f(e);
    }
}

/// Count handles that support a protocol (for `net` status).
pub fn count_protocol(g: &Guid) -> usize {
    let (hs, n) = locate_handles(g);
    if !hs.is_null() {
        free(hs as *mut u8);
    }
    n
}

/// Service-binding CreateChild: returns a new child handle, or null.
pub fn sb_create_child(sb: *mut u8) -> Handle {
    unsafe {
        let f: extern "efiapi" fn(*mut u8, *mut Handle) -> Status = core::mem::transmute(rd::<usize>(sb, 0));
        let mut child: Handle = ptr::null_mut();
        if f(sb, &mut child) == OK {
            child
        } else {
            ptr::null_mut()
        }
    }
}

pub fn sb_destroy_child(sb: *mut u8, child: Handle) {
    unsafe {
        let f: extern "efiapi" fn(*mut u8, Handle) -> Status = core::mem::transmute(rd::<usize>(sb, 8));
        f(sb, child);
    }
}

/// A NUL-terminated CHAR8 (ASCII) string from the firmware, as a slice.
pub unsafe fn cstr8<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(p, n)
}

pub fn raw_alloc(size: usize) -> *mut u8 {
    alloc(size)
}
pub fn raw_free(p: *mut u8) {
    free(p);
}
