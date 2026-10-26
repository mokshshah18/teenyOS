// Raw Linux i386 system calls. Every wrapper returns the kernel's raw result:
// non-negative on success, -errno on failure.

use core::arch::global_asm;

// cdecl trampoline: teeny_syscall(nr, a, b, c, d, e, f). Going through a real
// function sidesteps LLVM reserving ebx/esi/ebp for its own use.
global_asm!(
    ".globl teeny_syscall",
    "teeny_syscall:",
    "push %ebx",
    "push %esi",
    "push %edi",
    "push %ebp",
    "mov 20(%esp), %eax",
    "mov 24(%esp), %ebx",
    "mov 28(%esp), %ecx",
    "mov 32(%esp), %edx",
    "mov 36(%esp), %esi",
    "mov 40(%esp), %edi",
    "mov 44(%esp), %ebp",
    "int $0x80",
    "pop %ebp",
    "pop %edi",
    "pop %esi",
    "pop %ebx",
    "ret",
    options(att_syntax)
);

extern "C" {
    fn teeny_syscall(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize)
        -> isize;
}

#[inline(always)]
pub fn sc(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize) -> isize {
    unsafe { teeny_syscall(n, a, b, c, d, e, 0) }
}

pub const SYS_EXIT: usize = 1;
pub const SYS_FORK: usize = 2;
pub const SYS_READ: usize = 3;
pub const SYS_WRITE: usize = 4;
pub const SYS_OPEN: usize = 5;
pub const SYS_CLOSE: usize = 6;
pub const SYS_LINK: usize = 9;
pub const SYS_UNLINK: usize = 10;
pub const SYS_EXECVE: usize = 11;
pub const SYS_CHDIR: usize = 12;
pub const SYS_CHMOD: usize = 15;
pub const SYS_GETPID: usize = 20;
pub const SYS_MOUNT: usize = 21;
pub const SYS_SYNC: usize = 36;
pub const SYS_KILL: usize = 37;
pub const SYS_RENAME: usize = 38;
pub const SYS_MKDIR: usize = 39;
pub const SYS_RMDIR: usize = 40;
pub const SYS_PIPE: usize = 42;
pub const SYS_BRK: usize = 45;
pub const SYS_UMOUNT2: usize = 52;
pub const SYS_IOCTL: usize = 54;
pub const SYS_DUP2: usize = 63;
pub const SYS_SETSID: usize = 66;
pub const SYS_SETHOSTNAME: usize = 74;
pub const SYS_SYMLINK: usize = 83;
pub const SYS_READLINK: usize = 85;
pub const SYS_REBOOT: usize = 88;
pub const SYS_SYSLOG: usize = 103;
pub const SYS_WAIT4: usize = 114;
pub const SYS_SYSINFO: usize = 116;
pub const SYS_UNAME: usize = 122;
pub const SYS_RT_SIGACTION: usize = 174;
pub const SYS_GETCWD: usize = 183;
pub const SYS_STAT64: usize = 195;
pub const SYS_LSTAT64: usize = 196;
pub const SYS_GETUID32: usize = 199;
pub const SYS_GETDENTS64: usize = 220;
pub const SYS_FCNTL64: usize = 221;
pub const SYS_STATFS64: usize = 268;
// The kernel is built without the old 32-bit time_t calls, so only the
// time64 variants exist.
pub const SYS_CLOCK_GETTIME64: usize = 403;
pub const SYS_CLOCK_NANOSLEEP64: usize = 407;
pub const SYS_UTIMENSAT64: usize = 412;

pub const O_RDONLY: usize = 0;
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_CREAT: usize = 0o100;
pub const O_TRUNC: usize = 0o1000;
pub const O_APPEND: usize = 0o2000;
pub const O_NONBLOCK: usize = 0o4000;
pub const O_DIRECTORY: usize = 0o200000;
pub const O_CLOEXEC: usize = 0o2000000;

pub const ENOENT: isize = 2;
pub const EINTR: isize = 4;
pub const EEXIST: isize = 17;
pub const EXDEV: isize = 18;
pub const EISDIR: isize = 21;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFBLK: u32 = 0o060000;

pub const SIGHUP: usize = 1;
pub const SIGINT: usize = 2;
pub const SIGQUIT: usize = 3;
pub const SIGKILL: usize = 9;
pub const SIGTERM: usize = 15;
pub const SIGTSTP: usize = 20;
pub const SIGTTIN: usize = 21;
pub const SIGTTOU: usize = 22;

pub const WNOHANG: usize = 1;

/// Pointer to a NUL-terminated copy of `s`, valid until the next call that
/// shares the same scratch slot. Paths longer than 1023 bytes are cut off.
pub struct CStr {
    buf: [u8; 1024],
}

impl CStr {
    pub fn new(s: &[u8]) -> CStr {
        let mut c = CStr { buf: [0; 1024] };
        let n = s.len().min(1023);
        c.buf[..n].copy_from_slice(&s[..n]);
        c
    }
    pub fn p(&self) -> usize {
        self.buf.as_ptr() as usize
    }
}

pub fn exit(code: i32) -> ! {
    sc(SYS_EXIT, code as usize, 0, 0, 0, 0);
    loop {}
}

pub fn read(fd: i32, buf: &mut [u8]) -> isize {
    loop {
        let r = sc(
            SYS_READ,
            fd as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            0,
            0,
        );
        if r != -EINTR {
            return r;
        }
    }
}

pub fn write(fd: i32, buf: &[u8]) -> isize {
    sc(
        SYS_WRITE,
        fd as usize,
        buf.as_ptr() as usize,
        buf.len(),
        0,
        0,
    )
}

pub fn write_all(fd: i32, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let r = write(fd, buf);
        if r == -EINTR {
            continue;
        }
        if r <= 0 {
            return false;
        }
        buf = &buf[r as usize..];
    }
    true
}

pub fn open(path: &[u8], flags: usize, mode: usize) -> i32 {
    let c = CStr::new(path);
    sc(SYS_OPEN, c.p(), flags | O_CLOEXEC, mode, 0, 0) as i32
}

/// Like `open` but the descriptor survives execve (used for redirections).
pub fn open_inherit(path: &[u8], flags: usize, mode: usize) -> i32 {
    let c = CStr::new(path);
    sc(SYS_OPEN, c.p(), flags, mode, 0, 0) as i32
}

pub fn close(fd: i32) -> isize {
    sc(SYS_CLOSE, fd as usize, 0, 0, 0, 0)
}

pub fn dup2(a: i32, b: i32) -> isize {
    sc(SYS_DUP2, a as usize, b as usize, 0, 0, 0)
}

/// Duplicate `fd` onto the lowest free descriptor >= `min`, close-on-exec.
pub fn dup_high(fd: i32, min: i32) -> i32 {
    const F_DUPFD_CLOEXEC: usize = 1030;
    sc(
        SYS_FCNTL64,
        fd as usize,
        F_DUPFD_CLOEXEC,
        min as usize,
        0,
        0,
    ) as i32
}

pub fn pipe(fds: &mut [i32; 2]) -> isize {
    sc(SYS_PIPE, fds.as_mut_ptr() as usize, 0, 0, 0, 0)
}

pub fn fork() -> isize {
    sc(SYS_FORK, 0, 0, 0, 0, 0)
}

pub fn getpid() -> isize {
    sc(SYS_GETPID, 0, 0, 0, 0, 0)
}

pub fn getuid() -> isize {
    sc(SYS_GETUID32, 0, 0, 0, 0, 0)
}

/// Returns (pid, raw wait status).
pub fn wait4(pid: isize, flags: usize) -> (isize, i32) {
    let mut st: i32 = 0;
    loop {
        let r = sc(
            SYS_WAIT4,
            pid as usize,
            &mut st as *mut i32 as usize,
            flags,
            0,
            0,
        );
        if r != -EINTR {
            return (r, st);
        }
    }
}

/// Shell-style exit code from a raw wait status.
pub fn status_code(st: i32) -> i32 {
    if st & 0x7f == 0 {
        (st >> 8) & 0xff
    } else {
        128 + (st & 0x7f)
    }
}

pub fn kill(pid: isize, sig: usize) -> isize {
    sc(SYS_KILL, pid as usize, sig, 0, 0, 0)
}

pub fn chdir(path: &[u8]) -> isize {
    let c = CStr::new(path);
    sc(SYS_CHDIR, c.p(), 0, 0, 0, 0)
}

pub fn mkdir(path: &[u8], mode: usize) -> isize {
    let c = CStr::new(path);
    sc(SYS_MKDIR, c.p(), mode, 0, 0, 0)
}

pub fn rmdir(path: &[u8]) -> isize {
    let c = CStr::new(path);
    sc(SYS_RMDIR, c.p(), 0, 0, 0, 0)
}

pub fn unlink(path: &[u8]) -> isize {
    let c = CStr::new(path);
    sc(SYS_UNLINK, c.p(), 0, 0, 0, 0)
}

pub fn rename(a: &[u8], b: &[u8]) -> isize {
    let (ca, cb) = (CStr::new(a), CStr::new(b));
    sc(SYS_RENAME, ca.p(), cb.p(), 0, 0, 0)
}

pub fn link(a: &[u8], b: &[u8]) -> isize {
    let (ca, cb) = (CStr::new(a), CStr::new(b));
    sc(SYS_LINK, ca.p(), cb.p(), 0, 0, 0)
}

pub fn symlink(target: &[u8], path: &[u8]) -> isize {
    let (ca, cb) = (CStr::new(target), CStr::new(path));
    sc(SYS_SYMLINK, ca.p(), cb.p(), 0, 0, 0)
}

pub fn readlink(path: &[u8], buf: &mut [u8]) -> isize {
    let c = CStr::new(path);
    sc(
        SYS_READLINK,
        c.p(),
        buf.as_mut_ptr() as usize,
        buf.len(),
        0,
        0,
    )
}

pub fn chmod(path: &[u8], mode: usize) -> isize {
    let c = CStr::new(path);
    sc(SYS_CHMOD, c.p(), mode, 0, 0, 0)
}

/// Set atime and mtime of `path` to now.
pub fn touch_now(path: &[u8]) -> isize {
    const AT_FDCWD: isize = -100;
    let c = CStr::new(path);
    sc(SYS_UTIMENSAT64, AT_FDCWD as usize, c.p(), 0, 0, 0)
}

pub fn getcwd(buf: &mut [u8]) -> isize {
    sc(SYS_GETCWD, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0)
}

pub fn execve(path: &[u8], argv: *const usize, envp: *const usize) -> isize {
    let c = CStr::new(path);
    sc(SYS_EXECVE, c.p(), argv as usize, envp as usize, 0, 0)
}

pub fn mount(src: &[u8], dst: &[u8], fstype: &[u8], flags: usize, data: &[u8]) -> isize {
    let (a, b, c, d) = (
        CStr::new(src),
        CStr::new(dst),
        CStr::new(fstype),
        CStr::new(data),
    );
    let dp = if data.is_empty() { 0 } else { d.p() };
    sc(SYS_MOUNT, a.p(), b.p(), c.p(), flags, dp)
}

pub fn umount(path: &[u8]) -> isize {
    let c = CStr::new(path);
    sc(SYS_UMOUNT2, c.p(), 0, 0, 0, 0)
}

pub fn sync() {
    sc(SYS_SYNC, 0, 0, 0, 0, 0);
}

pub fn reboot(cmd: usize) -> isize {
    sc(SYS_REBOOT, 0xfee1dead, 672274793, cmd, 0, 0)
}

pub fn sethostname(name: &[u8]) -> isize {
    sc(SYS_SETHOSTNAME, name.as_ptr() as usize, name.len(), 0, 0, 0)
}

pub fn setsid() -> isize {
    sc(SYS_SETSID, 0, 0, 0, 0, 0)
}

pub fn ioctl(fd: i32, req: usize, arg: usize) -> isize {
    sc(SYS_IOCTL, fd as usize, req, arg, 0, 0)
}

pub fn getdents(fd: i32, buf: &mut [u8]) -> isize {
    sc(
        SYS_GETDENTS64,
        fd as usize,
        buf.as_mut_ptr() as usize,
        buf.len(),
        0,
        0,
    )
}

const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;

pub fn sleep_ms(ms: u64) {
    // struct __kernel_timespec { i64 tv_sec; i64 tv_nsec; }
    let mut ts = [(ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64];
    // Restart after a signal that did not kill us, with the time remaining.
    while sc(
        SYS_CLOCK_NANOSLEEP64,
        CLOCK_MONOTONIC,
        0,
        ts.as_ptr() as usize,
        ts.as_mut_ptr() as usize,
        0,
    ) == -EINTR
    {}
}

/// Seconds since the epoch (wall clock).
pub fn now() -> u64 {
    let mut ts = [0i64; 2];
    sc(
        SYS_CLOCK_GETTIME64,
        CLOCK_REALTIME,
        ts.as_mut_ptr() as usize,
        0,
        0,
        0,
    );
    ts[0] as u64
}

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

pub fn signal(sig: usize, handler: usize) {
    // struct sigaction { handler, flags, restorer, mask[2] }
    let act = [handler, 0usize, 0, 0, 0];
    sc(SYS_RT_SIGACTION, sig, act.as_ptr() as usize, 0, 8, 0);
}

/// struct stat64 as laid out by the i386 kernel (96 bytes, 4-byte aligned).
pub struct Stat {
    pub b: [u8; 96],
}

impl Stat {
    fn u32_at(&self, o: usize) -> u32 {
        u32::from_le_bytes([self.b[o], self.b[o + 1], self.b[o + 2], self.b[o + 3]])
    }
    pub fn mode(&self) -> u32 {
        self.u32_at(16)
    }
    pub fn nlink(&self) -> u32 {
        self.u32_at(20)
    }
    pub fn uid(&self) -> u32 {
        self.u32_at(24)
    }
    pub fn gid(&self) -> u32 {
        self.u32_at(28)
    }
    pub fn rdev(&self) -> u32 {
        self.u32_at(32)
    }
    pub fn size(&self) -> u64 {
        self.u32_at(44) as u64 | (self.u32_at(48) as u64) << 32
    }
    pub fn mtime(&self) -> u64 {
        self.u32_at(72) as u64
    }
    pub fn ino(&self) -> u64 {
        self.u32_at(88) as u64 | (self.u32_at(92) as u64) << 32
    }
    pub fn dev(&self) -> u64 {
        self.u32_at(0) as u64 | (self.u32_at(4) as u64) << 32
    }
    pub fn is_dir(&self) -> bool {
        self.mode() & S_IFMT == S_IFDIR
    }
    pub fn is_lnk(&self) -> bool {
        self.mode() & S_IFMT == S_IFLNK
    }
}

fn stat_with(n: usize, path: &[u8]) -> Option<Stat> {
    let c = CStr::new(path);
    let mut st = Stat { b: [0; 96] };
    if sc(n, c.p(), st.b.as_mut_ptr() as usize, 0, 0, 0) < 0 {
        None
    } else {
        Some(st)
    }
}

pub fn stat(path: &[u8]) -> Option<Stat> {
    stat_with(SYS_STAT64, path)
}

pub fn lstat(path: &[u8]) -> Option<Stat> {
    stat_with(SYS_LSTAT64, path)
}

// Terminal control.
pub const TCGETS: usize = 0x5401;
pub const TCSETS: usize = 0x5402;
pub const TIOCSCTTY: usize = 0x540e;
pub const TIOCGWINSZ: usize = 0x5413;

#[derive(Clone, Copy)]
pub struct Termios {
    pub b: [u32; 9],
}

impl Termios {
    pub fn get(fd: i32) -> Option<Termios> {
        let mut t = Termios { b: [0; 9] };
        if ioctl(fd, TCGETS, t.b.as_mut_ptr() as usize) < 0 {
            None
        } else {
            Some(t)
        }
    }
    pub fn set(&self, fd: i32) {
        ioctl(fd, TCSETS, self.b.as_ptr() as usize);
    }
    /// Byte-at-a-time input with no echo; signals still work unless `isig` is false.
    pub fn raw(&self, isig: bool) -> Termios {
        let mut t = *self;
        t.b[0] &= !(0x100 | 0x400 | 0x40); // ICRNL | IXON | INLCR
        t.b[3] &= !(0x2 | 0x8); // ICANON | ECHO
        if !isig {
            t.b[3] &= !0x1; // ISIG
        }
        // c_cc starts at byte 17: c_line is byte 16. VTIME = 5, VMIN = 6.
        let cc =
            unsafe { core::slice::from_raw_parts_mut((t.b.as_mut_ptr() as *mut u8).add(17), 19) };
        cc[5] = 0;
        cc[6] = 1;
        t
    }
}

/// (rows, cols) of the terminal on `fd`, with a fallback for serial lines.
pub fn winsize(fd: i32) -> (usize, usize) {
    let mut ws = [0u16; 4];
    if ioctl(fd, TIOCGWINSZ, ws.as_mut_ptr() as usize) < 0 || ws[0] == 0 || ws[1] == 0 {
        (24, 80)
    } else {
        (ws[0] as usize, ws[1] as usize)
    }
}

pub fn isatty(fd: i32) -> bool {
    Termios::get(fd).is_some()
}

pub fn strerror(e: isize) -> &'static [u8] {
    match -e {
        1 => b"Operation not permitted",
        2 => b"No such file or directory",
        3 => b"No such process",
        5 => b"I/O error",
        7 => b"Argument list too long",
        8 => b"Exec format error",
        9 => b"Bad file descriptor",
        10 => b"No child processes",
        12 => b"Out of memory",
        13 => b"Permission denied",
        16 => b"Device or resource busy",
        17 => b"File exists",
        18 => b"Cross-device link",
        19 => b"No such device",
        20 => b"Not a directory",
        21 => b"Is a directory",
        22 => b"Invalid argument",
        25 => b"Not a terminal",
        28 => b"No space left on device",
        30 => b"Read-only file system",
        36 => b"File name too long",
        38 => b"Function not implemented",
        39 => b"Directory not empty",
        40 => b"Too many symbolic links",
        _ => b"Error",
    }
}
