#![allow(dead_code)] // complete subsystem API; some entries are not yet exercised
// Cooperative green threads. With no kernel and boot services still live,
// teenyOS core runs one real thread of execution; this layer slices it into
// many. The desktop is the main loop and calls pump() each frame, which lets
// every other thread (a shell in a window, a background download) run until
// it next blocks on I/O and yields. That lets the blocking-style shell and
// applets run unchanged, each in its own window.

use crate::proc::Proc;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::cell::RefCell;

const STACK: usize = 256 * 1024;

#[derive(PartialEq, Clone, Copy)]
pub enum State {
    Ready,
    Blocked,
    Zombie,
}

pub struct Thread {
    pub id: u32,
    sp: u64,
    _stack: Vec<u8>,
    pub state: State,
    pub exit_code: i32,
    entry: Option<Box<dyn FnOnce()>>,
    pub proc: Proc,
    /// Set true by a thread each time it makes progress, so the scheduler
    /// knows whether a whole round was idle and it can sleep.
    pub progressed: bool,
}

struct Sched {
    threads: Vec<Thread>,
    cur: usize,       // index of the running thread (0 = desktop)
    main_sp: u64,     // the scheduler's own context
    next_id: u32,
}

// A single-threaded program, so one RefCell guards the whole scheduler.
// SAFETY: teenyOS core is strictly single-threaded (no SMP, no interrupts we
// enable), so this is never accessed concurrently.
struct SyncCell(RefCell<Option<Sched>>);
unsafe impl Sync for SyncCell {}
static SCHED: SyncCell = SyncCell(RefCell::new(None));

global_asm!(
    // switch(old_sp_out: *mut u64 [rdi], new_sp: u64 [rsi])
    ".globl teeny_switch",
    "teeny_switch:",
    "push %rbx",
    "push %rbp",
    "push %r12",
    "push %r13",
    "push %r14",
    "push %r15",
    "mov %rsp, (%rdi)",
    "mov %rsi, %rsp",
    "pop %r15",
    "pop %r14",
    "pop %r13",
    "pop %r12",
    "pop %rbp",
    "pop %rbx",
    "ret",
    options(att_syntax)
);

extern "sysv64" {
    fn teeny_switch(old_sp_out: *mut u64, new_sp: u64);
}

fn with<R>(f: impl FnOnce(&mut Sched) -> R) -> R {
    // Reentrancy is avoided by never yielding while holding this borrow.
    let mut b = SCHED.0.borrow_mut();
    f(b.as_mut().expect("sched not initialised"))
}

pub fn init(desktop_proc: Proc) {
    let mut s = Sched {
        threads: Vec::new(),
        cur: 0,
        main_sp: 0,
        next_id: 1,
    };
    // Thread 0 is the desktop: it runs on the original stack, no entry.
    s.threads.push(Thread {
        id: 0,
        sp: 0,
        _stack: Vec::new(),
        state: State::Ready,
        exit_code: 0,
        entry: None,
        proc: desktop_proc,
        progressed: false,
    });
    *SCHED.0.borrow_mut() = Some(s);
}

#[no_mangle]
extern "sysv64" fn teeny_thread_start() -> ! {
    let entry = with(|s| s.threads[s.cur].entry.take());
    if let Some(e) = entry {
        e();
    }
    exit(0);
}

/// Spawn a green thread. `proc` is its process context (fds, cwd, argv).
pub fn spawn(proc: Proc, entry: impl FnOnce() + 'static) -> u32 {
    let mut stack = vec![0u8; STACK];
    let top = stack.as_mut_ptr() as u64 + STACK as u64;
    // Lay out the initial stack so that, after teeny_switch pops 6 saved regs
    // and `ret`s into the trampoline, the trampoline runs with rsp bottom
    // byte == 8 (mod 16), the ABI's expectation at a function's first
    // instruction. The trampoline return slot must therefore be 16-aligned,
    // with an 8-byte pad above it.
    let mut sp = top & !0xf;
    unsafe {
        sp -= 8; // alignment pad
        sp -= 8;
        *(sp as *mut u64) = teeny_thread_start as *const () as u64; // ret target (16-aligned)
        sp -= 8 * 6; // r15,r14,r13,r12,rbp,rbx (values don't matter)
    }
    with(|s| {
        let id = s.next_id;
        s.next_id += 1;
        s.threads.push(Thread {
            id,
            sp,
            _stack: stack,
            state: State::Ready,
            exit_code: 0,
            entry: Some(Box::new(entry)),
            proc,
            progressed: false,
        });
        id
    })
}

/// Give other threads a turn. Called from inside a thread when it would block.
pub fn yield_now() {
    let (cur_idx, main_sp) = with(|s| (s.cur, s.main_sp));
    let cur_sp_ptr = with(|s| &mut s.threads[cur_idx].sp as *mut u64);
    unsafe {
        teeny_switch(cur_sp_ptr, main_sp);
    }
}

/// End the current thread.
pub fn exit(code: i32) -> ! {
    let cur_idx = with(|s| s.cur);
    with(|s| {
        s.threads[cur_idx].state = State::Zombie;
        s.threads[cur_idx].exit_code = code;
    });
    let main_sp = with(|s| s.main_sp);
    let mut dummy = 0u64;
    unsafe {
        teeny_switch(&mut dummy, main_sp);
    }
    unreachable!()
}

/// Run every ready non-desktop thread until it yields or exits. Returns true
/// if any thread made progress this round (so the caller keeps pumping rather
/// than sleeping). Called by the desktop once per frame.
pub fn pump() -> bool {
    let mut any = false;
    loop {
        // Find the next ready thread after index 0 (the desktop).
        let next = with(|s| {
            let n = s.threads.len();
            (1..n).find(|&i| s.threads[i].state == State::Ready)
        });
        let Some(i) = next else { break };
        with(|s| {
            s.cur = i;
            s.threads[i].progressed = false;
        });
        let new_sp = with(|s| s.threads[i].sp);
        let main_sp_ptr = with(|s| &mut s.main_sp as *mut u64);
        unsafe {
            teeny_switch(main_sp_ptr, new_sp);
        }
        with(|s| s.cur = 0);
        // Block this thread for the rest of the round so one busy thread can't
        // starve the frame; reap zombies.
        let progressed = with(|s| {
            let t = &mut s.threads[i];
            let p = t.progressed;
            if t.state == State::Ready {
                t.state = State::Blocked;
            }
            p
        });
        any |= progressed;
    }
    reap();
    // Re-arm all blocked threads for the next frame.
    with(|s| {
        for t in s.threads.iter_mut().skip(1) {
            if t.state == State::Blocked {
                t.state = State::Ready;
            }
        }
    });
    any
}

fn reap() {
    with(|s| {
        s.threads.retain(|t| t.id == 0 || t.state != State::Zombie);
    });
}

pub fn current_id() -> u32 {
    with(|s| s.threads[s.cur].id)
}

/// Access the running thread's process context.
pub fn with_proc<R>(f: impl FnOnce(&mut Proc) -> R) -> R {
    with(|s| {
        let i = s.cur;
        f(&mut s.threads[i].proc)
    })
}

pub fn mark_progress() {
    with(|s| {
        let i = s.cur;
        s.threads[i].progressed = true;
    });
}

/// How many non-desktop threads exist (for a jobs listing / shutdown).
pub fn thread_count() -> usize {
    with(|s| s.threads.len())
}

/// Wait for a specific thread to become a zombie, returning its exit code.
/// Yields while waiting. Returns None if there is no such thread.
pub fn join(id: u32) -> Option<i32> {
    loop {
        let r = with(|s| {
            match s.threads.iter().find(|t| t.id == id) {
                None => Some(None),
                Some(t) if t.state == State::Zombie => Some(Some(t.exit_code)),
                Some(_) => None,
            }
        });
        match r {
            Some(res) => return res,
            None => yield_now(),
        }
    }
}

/// Is there a live (non-zombie) thread with this id?
pub fn alive(id: u32) -> bool {
    with(|s| s.threads.iter().any(|t| t.id == id && t.state != State::Zombie))
}

/// Mark a thread dead so it is never scheduled again and gets reaped. Safe
/// only for a thread that is not the one currently running.
pub fn kill_thread(id: u32) {
    with(|s| {
        for t in s.threads.iter_mut() {
            if t.id == id {
                t.state = State::Zombie;
            }
        }
    });
}
