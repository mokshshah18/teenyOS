// Process entry, memory primitives, the heap, and buffered output.

use crate::sys;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;

global_asm!(
    ".globl _start",
    "_start:",
    "xor %rbp, %rbp",
    "mov %rsp, %rdi",
    "and $-16, %rsp",
    "call teeny_entry",
    options(att_syntax)
);

static mut ARGC: usize = 0;
static mut ARGV: *const *const u8 = core::ptr::null();
static mut ENVP: *const *const u8 = core::ptr::null();

#[no_mangle]
unsafe extern "C" fn teeny_entry(sp: *const usize) -> ! {
    ARGC = *sp;
    ARGV = sp.add(1) as *const *const u8;
    ENVP = ARGV.add(ARGC + 1);
    let code = crate::main();
    flush();
    sys::exit(code)
}

pub unsafe fn cstr<'a>(p: *const u8) -> &'a [u8] {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(p, n)
}

pub fn args() -> Vec<&'static [u8]> {
    let mut v = Vec::new();
    unsafe {
        for i in 0..ARGC {
            v.push(cstr(*ARGV.add(i)));
        }
    }
    v
}

pub fn env() -> Vec<&'static [u8]> {
    let mut v = Vec::new();
    unsafe {
        let mut p = ENVP;
        while !(*p).is_null() {
            v.push(cstr(*p));
            p = p.add(1);
        }
    }
    v
}

pub fn getenv(name: &[u8]) -> Option<&'static [u8]> {
    for e in env() {
        if e.len() > name.len() && e.starts_with(name) && e[name.len()] == b'=' {
            return Some(&e[name.len() + 1..]);
        }
    }
    None
}

pub fn envp_raw() -> *const usize {
    unsafe { ENVP as *const usize }
}

// The compiler lowers copies and fills to these symbols. They are written in
// assembly so LLVM cannot turn them back into calls to themselves.
global_asm!(
    ".globl memcpy",
    "memcpy:",
    "mov %rdi, %rax",
    "mov %rdx, %rcx",
    "rep movsb",
    "ret",
    ".globl memmove",
    "memmove:",
    "mov %rdi, %rax",
    "mov %rdx, %rcx",
    "cmp %rsi, %rdi",
    "jbe 1f",
    "lea -1(%rsi,%rcx), %rsi",
    "lea -1(%rdi,%rcx), %rdi",
    "std",
    "1:",
    "rep movsb",
    "cld",
    "ret",
    ".globl memset",
    "memset:",
    "mov %rdi, %r9",
    "mov %esi, %eax",
    "mov %rdx, %rcx",
    "rep stosb",
    "mov %r9, %rax",
    "ret",
    ".globl strlen",
    "strlen:",
    "xor %eax, %eax",
    "mov $-1, %rcx",
    "repne scasb",
    "not %rcx",
    "dec %rcx",
    "mov %rcx, %rax",
    "ret",
    options(att_syntax)
);

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let (x, y) = (
            core::ptr::read_volatile(a.add(i)),
            core::ptr::read_volatile(b.add(i)),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    memcmp(a, b, n)
}

// Heap: power-of-two size classes carved out of brk, each with a free list.
// Freed blocks are reused by later requests of the same class, so a long-lived
// shell does not grow without bound.
struct Heap;

const CLASSES: usize = 28;
static mut FREE: [usize; CLASSES] = [0; CLASSES];
static mut BRK_CUR: usize = 0;
static mut BRK_END: usize = 0;

fn class_of(size: usize) -> usize {
    let mut c = 4; // 16 bytes minimum
    while (1usize << c) < size {
        c += 1;
    }
    c
}

unsafe fn grow(bytes: usize, align: usize) -> *mut u8 {
    if BRK_CUR == 0 {
        BRK_CUR = sys::sc(sys::SYS_BRK, 0, 0, 0, 0, 0) as usize;
        BRK_END = BRK_CUR;
    }
    let start = (BRK_CUR + align - 1) & !(align - 1);
    let end = start + bytes;
    if end > BRK_END {
        let want = (end + 0xffff) & !0xffff;
        let got = sys::sc(sys::SYS_BRK, want, 0, 0, 0, 0) as usize;
        if got < end {
            return core::ptr::null_mut();
        }
        BRK_END = got;
    }
    BRK_CUR = end;
    start as *mut u8
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let c = class_of(l.size().max(l.align()));
        if c >= CLASSES {
            return core::ptr::null_mut();
        }
        let head = FREE[c];
        if head != 0 {
            FREE[c] = *(head as *const usize);
            return head as *mut u8;
        }
        // Blocks are aligned to their own size (capped at a page), which
        // satisfies any alignment up to the block size.
        grow(1 << c, (1usize << c).min(4096))
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let c = class_of(l.size().max(l.align()));
        *(p as *mut usize) = FREE[c];
        FREE[c] = p as usize;
    }
}

#[global_allocator]
static HEAP: Heap = Heap;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    flush();
    sys::write(2, b"teeny: internal error\n");
    sys::exit(70)
}

// Buffered stdout. Anything interactive must call flush() before blocking.
struct OutBuf {
    buf: [u8; 4096],
    len: usize,
}

static mut OUT: OutBuf = OutBuf {
    buf: [0; 4096],
    len: 0,
};

fn outbuf() -> &'static mut OutBuf {
    // Single-threaded program: there is never more than one live reference.
    unsafe { &mut *core::ptr::addr_of_mut!(OUT) }
}

pub fn out(s: &[u8]) {
    let o = outbuf();
    if o.len + s.len() > o.buf.len() {
        flush();
        if s.len() > o.buf.len() {
            sys::write_all(1, s);
            return;
        }
    }
    o.buf[o.len..o.len + s.len()].copy_from_slice(s);
    o.len += s.len();
}

pub fn flush() {
    let o = outbuf();
    if o.len > 0 {
        let n = o.len;
        o.len = 0;
        sys::write_all(1, &o.buf[..n]);
    }
}

pub fn err(s: &[u8]) {
    flush();
    sys::write_all(2, s);
}
