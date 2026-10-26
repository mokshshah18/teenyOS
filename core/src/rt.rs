#![allow(dead_code)] // low-level runtime API kept complete even where unused
// Global allocator over the firmware pool, memory intrinsics, and the panic
// handler. No libc, no std.

use crate::uefi;
use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;

// A slab allocator over one big block taken from the firmware pool. We take
// the block once (FreePool under churn proved unreliable on OVMF) and manage
// it ourselves: power-of-two size classes with free lists, and a bump pointer
// for fresh blocks. Freed blocks are reused, so a long session is bounded.
use core::sync::atomic::{AtomicUsize, Ordering};

const CLASSES: usize = 32;
const ARENA: usize = 64 * 1024 * 1024;

static FREE: [AtomicUsize; CLASSES] = {
    const Z: AtomicUsize = AtomicUsize::new(0);
    [Z; CLASSES]
};
static BASE: AtomicUsize = AtomicUsize::new(0);
static BUMP: AtomicUsize = AtomicUsize::new(0);
static END: AtomicUsize = AtomicUsize::new(0);

fn class_of(size: usize) -> usize {
    let mut c = 4; // 16 bytes minimum
    while (1usize << c) < size {
        c += 1;
    }
    c
}

struct Slab;

unsafe impl GlobalAlloc for Slab {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if END.load(Ordering::Relaxed) == 0 {
            // Take the largest block the firmware will give, down to 8 MB.
            let mut sz = ARENA;
            let mut base = core::ptr::null_mut();
            while sz >= 8 * 1024 * 1024 {
                base = uefi::alloc(sz);
                if !base.is_null() {
                    break;
                }
                sz /= 2;
            }
            if base.is_null() {
                return core::ptr::null_mut();
            }
            BASE.store(base as usize, Ordering::Relaxed);
            BUMP.store(base as usize, Ordering::Relaxed);
            END.store(base as usize + sz, Ordering::Relaxed);
        }
        let need = l.size().max(l.align()).max(1);
        let c = class_of(need);
        if c >= CLASSES {
            return core::ptr::null_mut();
        }
        let head = FREE[c].load(Ordering::Relaxed);
        if head != 0 {
            FREE[c].store(*(head as *const usize), Ordering::Relaxed);
            return head as *mut u8;
        }
        let block = 1usize << c;
        let align = block.min(4096);
        let mut p = BUMP.load(Ordering::Relaxed);
        p = (p + align - 1) & !(align - 1);
        let np = p + block;
        if np > END.load(Ordering::Relaxed) {
            return core::ptr::null_mut();
        }
        BUMP.store(np, Ordering::Relaxed);
        p as *mut u8
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let need = l.size().max(l.align()).max(1);
        let c = class_of(need);
        if c >= CLASSES {
            return;
        }
        *(p as *mut usize) = FREE[c].load(Ordering::Relaxed);
        FREE[c].store(p as usize, Ordering::Relaxed);
    }
}

#[global_allocator]
static HEAP: Slab = Slab;

/// (bytes handed out so far, total arena size). Approximate: the bump pointer
/// minus the base, which counts freed-and-reused blocks as still used.
pub fn heap_stats() -> (usize, usize) {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return (0, 0);
    }
    let total = END.load(Ordering::Relaxed) - base;
    (BUMP.load(Ordering::Relaxed) - base, total)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    uefi::panic_flash(0x00ff00ff);
    uefi::con_print("\ncore panic");
    if let Some(l) = info.location() {
        uefi::con_print(" at ");
        uefi::con_print(l.file());
    }
    uefi::con_print("\n");
    loop {
        uefi::stall(1_000_000);
    }
}

// The compiler lowers struct copies and array fills to these. On
// x86_64-unknown-uefi the C ABI is Win64 (args in rcx, rdx, r8), and rdi/rsi
// are callee-saved -- so these must preserve rdi/rsi and read args from the
// Win64 registers.
global_asm!(
    ".globl memcpy",
    "memcpy:",
    "mov %rcx, %rax",
    "mov %rsi, %r9",
    "mov %rdi, %r10",
    "mov %rcx, %rdi",
    "mov %rdx, %rsi",
    "mov %r8, %rcx",
    "rep movsb",
    "mov %r9, %rsi",
    "mov %r10, %rdi",
    "ret",
    ".globl memmove",
    "memmove:",
    "mov %rcx, %rax",
    "mov %rsi, %r9",
    "mov %rdi, %r10",
    "mov %rcx, %rdi",
    "mov %rdx, %rsi",
    "mov %r8, %rcx",
    "cmp %rsi, %rdi",
    "jbe 1f",
    "lea -1(%rsi,%rcx), %rsi",
    "lea -1(%rdi,%rcx), %rdi",
    "std",
    "1:",
    "rep movsb",
    "cld",
    "mov %r9, %rsi",
    "mov %r10, %rdi",
    "ret",
    ".globl memset",
    "memset:",
    "mov %rcx, %r9",
    "mov %rdi, %r10",
    "mov %rcx, %rdi",
    "mov %edx, %eax",
    "mov %r8, %rcx",
    "rep stosb",
    "mov %r10, %rdi",
    "mov %r9, %rax",
    "ret",
    ".globl memcmp",
    "memcmp:",
    "xor %eax, %eax",
    "test %r8, %r8",
    "jz 2f",
    "1:",
    "movzbl (%rcx), %eax",
    "movzbl (%rdx), %r9d",
    "sub %r9d, %eax",
    "jnz 2f",
    "inc %rcx",
    "inc %rdx",
    "dec %r8",
    "jnz 1b",
    "xor %eax, %eax",
    "2:",
    "ret",
    ".globl bcmp",
    "bcmp:",
    "jmp memcmp",
    ".globl strlen",
    "strlen:",
    "mov %rdi, %r10",
    "mov %rcx, %rdi",
    "xor %eax, %eax",
    "mov $-1, %rcx",
    "repne scasb",
    "not %rcx",
    "dec %rcx",
    "mov %rcx, %rax",
    "mov %r10, %rdi",
    "ret",
    options(att_syntax)
);

// --- application I/O: an applet's stdout/stderr are its thread's fd 1/2 ---

use crate::sched;
use crate::sysb as sys;
use alloc::vec::Vec;

pub fn out(s: &[u8]) {
    sys::write_all(1, s);
}

pub fn err(s: &[u8]) {
    sys::write_all(2, s);
}

pub fn flush() {}

pub unsafe fn cstr<'a>(p: *const u8) -> &'a [u8] {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(p, n)
}

pub fn args() -> Vec<Vec<u8>> {
    sched::with_proc(|p| p.argv.clone())
}

pub fn getenv(name: &[u8]) -> Option<Vec<u8>> {
    sched::with_proc(|p| p.getenv(name).map(|v| v.to_vec()))
}

pub fn env() -> Vec<Vec<u8>> {
    sched::with_proc(|p| {
        p.env.iter().map(|(n, v)| {
            let mut e = n.clone();
            e.push(b'=');
            e.extend_from_slice(v);
            e
        }).collect()
    })
}
