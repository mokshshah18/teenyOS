// Global allocator over the firmware pool, memory intrinsics, and the panic
// handler. No libc, no std.

use crate::uefi;
use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;

struct Fw;

// The firmware pool is only 8-byte aligned. For larger alignment we over-
// allocate and stash the real base in the 8 bytes before the aligned pointer.
unsafe impl GlobalAlloc for Fw {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let align = l.align().max(8);
        if align <= 8 {
            return uefi::alloc(l.size().max(1));
        }
        let raw = uefi::alloc(l.size() + align + 8);
        if raw.is_null() {
            return raw;
        }
        let aligned = ((raw as usize + 8 + align - 1) & !(align - 1)) as *mut u8;
        *(aligned.sub(8) as *mut usize) = raw as usize;
        aligned
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.align().max(8) <= 8 {
            uefi::free(p);
        } else {
            uefi::free(*(p.sub(8) as *const usize) as *mut u8);
        }
    }
}

#[global_allocator]
static HEAP: Fw = Fw;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
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

// The compiler lowers struct copies and array fills to these.
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
    ".globl memcmp",
    "memcmp:",
    "xor %eax, %eax",
    "mov %rdx, %rcx",
    "test %rcx, %rcx",
    "jz 2f",
    "1:",
    "movzbl (%rdi), %eax",
    "movzbl (%rsi), %r8d",
    "sub %r8d, %eax",
    "jnz 2f",
    "inc %rdi",
    "inc %rsi",
    "dec %rcx",
    "jnz 1b",
    "2:",
    "ret",
    ".globl bcmp",
    "bcmp:",
    "jmp memcmp",
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
