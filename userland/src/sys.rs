// Raw Linux x86_64 system calls. Every wrapper returns the kernel's raw
// result: non-negative on success, -errno on failure.

use core::arch::asm;

#[inline(always)]
pub fn sc(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize) -> isize {
    sc6(n, a, b, c, d, e, 0)
}

#[inline(always)]
pub fn sc6(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize) -> isize {
    let r: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") n as isize => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            in("r10") d,
            in("r8") e,
            in("r9") f,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    r
}

pub const SYS_READ: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_OPEN: usize = 2;
pub const SYS_CLOSE: usize = 3;
pub const SYS_STAT: usize = 4;
pub const SYS_LSTAT: usize = 6;
pub const SYS_POLL: usize = 7;
pub const SYS_MMAP: usize = 9;
pub const SYS_BRK: usize = 12;
pub const SYS_RT_SIGACTION: usize = 13;
pub const SYS_IOCTL: usize = 16;
pub const SYS_PREAD: usize = 17;
pub const SYS_PWRITE: usize = 18;
pub const SYS_PIPE: usize = 22;
pub const SYS_DUP2: usize = 33;
pub const SYS_GETPID: usize = 39;
pub const SYS_FORK: usize = 57;
pub const SYS_EXECVE: usize = 59;
pub const SYS_EXIT: usize = 60;
pub const SYS_WAIT4: usize = 61;
pub const SYS_KILL: usize = 62;
pub const SYS_UNAME: usize = 63;
pub const SYS_FCNTL: usize = 72;
pub const SYS_FSYNC: usize = 74;
pub const SYS_GETCWD: usize = 79;
pub const SYS_CHDIR: usize = 80;
pub const SYS_RENAME: usize = 82;
pub const SYS_MKDIR: usize = 83;
pub const SYS_RMDIR: usize = 84;
pub const SYS_LINK: usize = 86;
pub const SYS_UNLINK: usize = 87;
pub const SYS_SYMLINK: usize = 88;
pub const SYS_READLINK: usize = 89;
pub const SYS_CHMOD: usize = 90;
pub const SYS_SYSINFO: usize = 99;
pub const SYS_GETUID: usize = 102;
pub const SYS_SYSLOG: usize = 103;
pub const SYS_SETSID: usize = 112;
pub const SYS_STATFS: usize = 137;
pub const SYS_SYNC: usize = 162;
pub const SYS_MOUNT: usize = 165;
pub const SYS_UMOUNT2: usize = 166;
pub const SYS_REBOOT: usize = 169;
pub const SYS_SETHOSTNAME: usize = 170;
pub const SYS_GETDENTS64: usize = 217;
pub const SYS_CLOCK_GETTIME: usize = 228;
pub const SYS_CLOCK_NANOSLEEP: usize = 230;
pub const SYS_UTIMENSAT: usize = 280;

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
        SYS_FCNTL,
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
    sc(SYS_GETUID, 0, 0, 0, 0, 0)
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
    sc(SYS_UTIMENSAT, AT_FDCWD as usize, c.p(), 0, 0, 0)
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
        SYS_CLOCK_NANOSLEEP,
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
        SYS_CLOCK_GETTIME,
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
    // struct sigaction { handler, flags, restorer, mask }
    let act = [handler, 0usize, 0, 0, 0];
    sc(SYS_RT_SIGACTION, sig, act.as_ptr() as usize, 0, 8, 0);
}

/// struct stat as laid out by the x86_64 kernel (144 bytes).
pub struct Stat {
    pub b: [u8; 144],
}

impl Stat {
    fn u32_at(&self, o: usize) -> u32 {
        u32::from_le_bytes([self.b[o], self.b[o + 1], self.b[o + 2], self.b[o + 3]])
    }
    fn u64_at(&self, o: usize) -> u64 {
        self.u32_at(o) as u64 | (self.u32_at(o + 4) as u64) << 32
    }
    pub fn dev(&self) -> u64 {
        self.u64_at(0)
    }
    pub fn ino(&self) -> u64 {
        self.u64_at(8)
    }
    pub fn nlink(&self) -> u32 {
        self.u64_at(16) as u32
    }
    pub fn mode(&self) -> u32 {
        self.u32_at(24)
    }
    pub fn uid(&self) -> u32 {
        self.u32_at(28)
    }
    pub fn gid(&self) -> u32 {
        self.u32_at(32)
    }
    pub fn rdev(&self) -> u32 {
        self.u64_at(40) as u32
    }
    pub fn size(&self) -> u64 {
        self.u64_at(48)
    }
    pub fn mtime(&self) -> u64 {
        self.u64_at(88)
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
    let mut st = Stat { b: [0; 144] };
    if sc(n, c.p(), st.b.as_mut_ptr() as usize, 0, 0, 0) < 0 {
        None
    } else {
        Some(st)
    }
}

pub fn stat(path: &[u8]) -> Option<Stat> {
    stat_with(SYS_STAT, path)
}

pub fn lstat(path: &[u8]) -> Option<Stat> {
    stat_with(SYS_LSTAT, path)
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

// ---------------------------------------------------------------- desktop

pub const POLLIN: i16 = 1;

#[repr(C)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub fn poll(fds: &mut [PollFd], timeout_ms: i32) -> isize {
    sc(SYS_POLL, fds.as_mut_ptr() as usize, fds.len(), timeout_ms as isize as usize, 0, 0)
}

/// Map `len` bytes of `fd` shared and writable. Returns null on failure.
pub fn mmap_shared(fd: i32, len: usize) -> *mut u8 {
    const PROT_RW: usize = 3;
    const MAP_SHARED: usize = 1;
    let r = sc6(SYS_MMAP, 0, len, PROT_RW, MAP_SHARED, fd as usize, 0);
    if r < 0 && r > -4096 {
        core::ptr::null_mut()
    } else {
        r as *mut u8
    }
}

/// Milliseconds on a clock that never jumps.
pub fn now_ms() -> u64 {
    let mut ts = [0i64; 2];
    sc(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC, ts.as_mut_ptr() as usize, 0, 0, 0);
    ts[0] as u64 * 1000 + ts[1] as u64 / 1_000_000
}

pub fn set_nonblock(fd: i32) {
    const F_GETFL: usize = 3;
    const F_SETFL: usize = 4;
    let fl = sc(SYS_FCNTL, fd as usize, F_GETFL, 0, 0, 0);
    if fl >= 0 {
        sc(SYS_FCNTL, fd as usize, F_SETFL, fl as usize | O_NONBLOCK, 0, 0);
    }
}

pub const TIOCSWINSZ: usize = 0x5414;
const TIOCGPTN: usize = 0x80045430;
const TIOCSPTLCK: usize = 0x40045431;
// Console: text or graphics mode, and the font the kernel draws with.
pub const KDSETMODE: usize = 0x4b3a;
pub const KD_TEXT: usize = 0;
pub const KD_GRAPHICS: usize = 1;
pub const KDFONTOP: usize = 0x4b72;

pub fn set_winsize(fd: i32, rows: usize, cols: usize) {
    let ws = [rows as u16, cols as u16, 0, 0];
    ioctl(fd, TIOCSWINSZ, ws.as_ptr() as usize);
}

/// Open a pseudo-terminal pair: (master fd, slave path).
pub fn openpty() -> Option<(i32, alloc::vec::Vec<u8>)> {
    let m = open(b"/dev/ptmx", O_RDWR | 0o400000 /* O_NOCTTY */, 0);
    if m < 0 {
        return None;
    }
    let zero: i32 = 0;
    let mut n: u32 = 0;
    if ioctl(m, TIOCSPTLCK, &zero as *const i32 as usize) < 0 || ioctl(m, TIOCGPTN, &mut n as *mut u32 as usize) < 0 {
        close(m);
        return None;
    }
    let mut path = b"/dev/pts/".to_vec();
    path.extend_from_slice(&crate::util::num(n as u64));
    Some((m, path))
}
