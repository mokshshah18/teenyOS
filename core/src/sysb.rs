// The `sys` surface the shell and applets call. On the Linux edition these are
// real system calls; here they run against the RAM filesystem, the per-thread
// fd table, the terminal channels and the cooperative scheduler. Blocking
// reads yield to the desktop and resume next frame.

use crate::proc::{Fd, Pipe};
use crate::{sched, tty, uefi, vfs};
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

pub const O_RDONLY: usize = 0;
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_CREAT: usize = 0o100;
pub const O_EXCL: usize = 0o200;
pub const O_TRUNC: usize = 0o1000;
pub const O_APPEND: usize = 0o2000;
pub const O_DIRECTORY: usize = 0o200000;

pub const S_IFMT: u32 = vfs::S_IFMT;
pub const S_IFDIR: u32 = vfs::S_IFDIR;
pub const S_IFREG: u32 = vfs::S_IFREG;
pub const S_IFLNK: u32 = vfs::S_IFLNK;

pub struct Stat {
    pub mode: u32,
    pub size: u64,
    pub mtime: u64,
    pub ino: u64,
    pub nlink: u32,
}

impl Stat {
    pub fn mode(&self) -> u32 {
        self.mode
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn mtime(&self) -> u64 {
        self.mtime
    }
    pub fn ino(&self) -> u64 {
        self.ino
    }
    pub fn nlink(&self) -> u32 {
        self.nlink
    }
    pub fn uid(&self) -> u32 {
        0
    }
    pub fn gid(&self) -> u32 {
        0
    }
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
    pub fn is_lnk(&self) -> bool {
        self.mode & S_IFMT == S_IFLNK
    }
}

fn to_stat(s: vfs::Stat) -> Stat {
    Stat { mode: s.mode, size: s.size, mtime: s.mtime, ino: s.ino, nlink: s.nlink }
}

pub fn stat(path: &[u8]) -> Option<Stat> {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::lookup(&cwd, path).ok().map(|i| to_stat(vfs::stat_idx(i)))
}

pub fn lstat(path: &[u8]) -> Option<Stat> {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::lookup_nofollow(&cwd, path).ok().map(|i| to_stat(vfs::stat_idx(i)))
}

pub fn open(path: &[u8], flags: usize, mode: usize) -> i32 {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    let idx = if flags & O_CREAT != 0 {
        match vfs::create(&cwd, path, mode as u32, flags & O_EXCL != 0) {
            Ok(i) => i,
            Err(e) => return e,
        }
    } else {
        match vfs::lookup(&cwd, path) {
            Ok(i) => i,
            Err(e) => return e,
        }
    };
    let writable = flags & (O_WRONLY | O_RDWR) != 0;
    if flags & O_TRUNC != 0 && writable {
        vfs::truncate(idx);
    }
    let append = flags & O_APPEND != 0;
    let pos = if append { vfs::file_len(idx) } else { 0 };
    sched::with_proc(|p| p.alloc_fd(Fd::File { idx, pos, writable, append }))
}

pub fn open_inherit(path: &[u8], flags: usize, mode: usize) -> i32 {
    open(path, flags, mode)
}

pub fn close(fd: i32) -> isize {
    sched::with_proc(|p| {
        if let Some(slot) = p.fds.get_mut(fd as usize) {
            if let Some(f) = slot.take() {
                match f {
                    Fd::PipeWrite(r) => r.borrow_mut().writers -= 1,
                    Fd::PipeRead(r) => r.borrow_mut().readers -= 1,
                    _ => {}
                }
                return 0;
            }
        }
        -9
    })
}

pub fn read(fd: i32, buf: &mut [u8]) -> isize {
    loop {
        let r = sched::with_proc(|p| {
            let f = match p.get(fd).cloned() {
                Some(f) => f,
                None => return Err(-9isize),
            };
            match f {
                Fd::Tty(id) => {
                    let mut n = 0;
                    while n < buf.len() {
                        match tty::read_input(id) {
                            Some(b) => {
                                buf[n] = b;
                                n += 1;
                            }
                            None => break,
                        }
                    }
                    Ok(n as isize)
                }
                Fd::File { idx, pos, .. } => {
                    let n = vfs::read_at(idx, pos, buf);
                    if let Some(Some(Fd::File { pos: pp, .. })) = p.fds.get_mut(fd as usize) {
                        *pp = pos + n;
                    }
                    Ok(n as isize)
                }
                Fd::PipeRead(r) => {
                    let mut b = r.borrow_mut();
                    if b.buf.is_empty() {
                        if b.writers == 0 {
                            return Ok(0);
                        }
                        return Err(isize::MIN); // would block
                    }
                    let n = buf.len().min(b.buf.len());
                    buf[..n].copy_from_slice(&b.buf[..n]);
                    b.buf.drain(..n);
                    Ok(n as isize)
                }
                Fd::Null => Ok(0),
                Fd::PipeWrite(_) => Err(-9),
            }
        });
        match r {
            Ok(n) if n == 0 && is_tty(fd) => {
                // Interactive stdin with nothing queued: yield and retry.
                sched::yield_now();
            }
            Ok(n) => return n,
            Err(e) if e == isize::MIN => sched::yield_now(),
            Err(e) => return e,
        }
    }
}

fn is_tty(fd: i32) -> bool {
    sched::with_proc(|p| matches!(p.get(fd), Some(Fd::Tty(_))))
}

pub fn write(fd: i32, buf: &[u8]) -> isize {
    sched::mark_progress();
    sched::with_proc(|p| {
        let f = match p.get(fd).cloned() {
            Some(f) => f,
            None => return -9,
        };
        match f {
            Fd::Tty(id) => {
                tty::write(id, buf);
                buf.len() as isize
            }
            Fd::File { idx, pos, writable, append } => {
                if !writable {
                    return -9;
                }
                let at = if append { vfs::file_len(idx) } else { pos };
                let r = vfs::write_at(idx, at, buf);
                if r >= 0 {
                    if let Some(Some(Fd::File { pos: pp, .. })) = p.fds.get_mut(fd as usize) {
                        *pp = at + buf.len();
                    }
                }
                r as isize
            }
            Fd::PipeWrite(r) => {
                r.borrow_mut().buf.extend_from_slice(buf);
                buf.len() as isize
            }
            Fd::Null => buf.len() as isize,
            Fd::PipeRead(_) => -9,
        }
    })
}

pub fn write_all(fd: i32, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        let r = write(fd, buf);
        if r <= 0 {
            return false;
        }
        buf = &buf[r as usize..];
    }
    true
}

pub fn dup2(old: i32, new: i32) -> isize {
    sched::with_proc(|p| {
        let f = match p.get(old).cloned() {
            Some(f) => f,
            None => return -9,
        };
        bump(&f);
        while p.fds.len() <= new as usize {
            p.fds.push(None);
        }
        p.fds[new as usize] = Some(f);
        new as isize
    })
}

pub fn dup_high(fd: i32, min: i32) -> i32 {
    sched::with_proc(|p| {
        let f = match p.get(fd).cloned() {
            Some(f) => f,
            None => return -9,
        };
        bump(&f);
        while (p.fds.len() as i32) < min {
            p.fds.push(None);
        }
        for i in (min as usize)..p.fds.len() {
            if p.fds[i].is_none() {
                p.fds[i] = Some(f);
                return i as i32;
            }
        }
        p.fds.push(Some(f));
        (p.fds.len() - 1) as i32
    })
}

fn bump(f: &Fd) {
    match f {
        Fd::PipeWrite(r) => r.borrow_mut().writers += 1,
        Fd::PipeRead(r) => r.borrow_mut().readers += 1,
        _ => {}
    }
}

pub fn pipe(fds: &mut [i32; 2]) -> isize {
    let p = Rc::new(RefCell::new(Pipe { buf: Vec::new(), writers: 1, readers: 1 }));
    sched::with_proc(|pr| {
        fds[0] = pr.alloc_fd(Fd::PipeRead(p.clone()));
        fds[1] = pr.alloc_fd(Fd::PipeWrite(p.clone()));
    });
    0
}

pub fn getpid() -> isize {
    sched::current_id() as isize
}

pub fn getuid() -> isize {
    0
}

pub fn chdir(path: &[u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    match vfs::lookup(&cwd, path) {
        Ok(i) if vfs::is_dir(i) => {
            let abs = abspath(&cwd, path);
            sched::with_proc(|p| p.cwd = abs);
            0
        }
        Ok(_) => -20,
        Err(e) => e as isize,
    }
}

/// Canonicalise path against cwd (resolving . and ..), textually.
pub fn abspath(cwd: &[u8], path: &[u8]) -> Vec<u8> {
    let mut comps: Vec<&[u8]> = Vec::new();
    if path.first() != Some(&b'/') {
        for c in cwd.split(|&b| b == b'/') {
            if !c.is_empty() {
                comps.push(c);
            }
        }
    }
    for c in path.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                comps.pop();
            }
            _ => comps.push(c),
        }
    }
    let mut out = Vec::new();
    for c in comps {
        out.push(b'/');
        out.extend_from_slice(c);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

pub fn getcwd(buf: &mut [u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    let n = cwd.len().min(buf.len() - 1);
    buf[..n].copy_from_slice(&cwd[..n]);
    buf[n] = 0;
    (n + 1) as isize
}

pub fn mkdir(path: &[u8], mode: usize) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::mkdir_at(&cwd, path, mode as u32) as isize
}

pub fn rmdir(path: &[u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::rmdir(&cwd, path) as isize
}

pub fn unlink(path: &[u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::unlink(&cwd, path) as isize
}

pub fn rename(a: &[u8], b: &[u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::rename(&cwd, a, b) as isize
}

pub fn symlink(target: &[u8], path: &[u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    vfs::symlink(&cwd, target, path) as isize
}

pub fn readlink(path: &[u8], buf: &mut [u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    match vfs::lookup_nofollow(&cwd, path).ok().and_then(vfs::readlink) {
        Some(t) => {
            let n = t.len().min(buf.len());
            buf[..n].copy_from_slice(&t[..n]);
            n as isize
        }
        None => -2,
    }
}

pub fn chmod(path: &[u8], mode: usize) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    match vfs::lookup(&cwd, path) {
        Ok(i) => {
            vfs::chmod(i, mode as u32);
            0
        }
        Err(e) => e as isize,
    }
}

pub fn touch_now(path: &[u8]) -> isize {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    match vfs::lookup(&cwd, path) {
        Ok(i) => {
            vfs::touch(i);
            0
        }
        Err(e) => e as isize,
    }
}

/// Directory entries excluding "." and "..".
pub fn list_dir(path: &[u8]) -> Result<Vec<Vec<u8>>, isize> {
    let cwd = sched::with_proc(|p| p.cwd.clone());
    let idx = vfs::lookup(&cwd, path).map_err(|e| e as isize)?;
    vfs::readdir(idx).ok_or(-20)
}

pub fn now() -> u64 {
    uefi::wall_time()
}

static MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Monotonic-ish milliseconds. We have no fine clock without leaving boot
/// services, so this advances a counter; good enough for UI timing and
/// relative measurements.
pub fn now_ms() -> u64 {
    MS.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn tick_ms(delta: u64) {
    MS.fetch_add(delta, core::sync::atomic::Ordering::Relaxed);
}

pub fn sleep_ms(ms: u64) {
    let end = now_ms() + ms;
    while now_ms() < end {
        sched::yield_now();
    }
}

pub fn isatty(fd: i32) -> bool {
    is_tty(fd)
}

pub fn winsize(fd: i32) -> (usize, usize) {
    match sched::with_proc(|p| p.get(fd).cloned()) {
        Some(Fd::Tty(id)) => {
            let (c, r) = tty::size(id);
            (r, c)
        }
        _ => (tty::DEF_ROWS, tty::DEF_COLS),
    }
}

pub fn exit(code: i32) -> ! {
    sched::exit(code)
}

pub fn status_code(st: i32) -> i32 {
    st & 0xff
}

/// Terminal mode control is a no-op: the tty already delivers raw keystrokes.
#[derive(Clone, Copy)]
pub struct Termios;

impl Termios {
    pub fn get(fd: i32) -> Option<Termios> {
        if is_tty(fd) {
            Some(Termios)
        } else {
            None
        }
    }
    pub fn set(&self, _fd: i32) {}
    pub fn raw(&self, _isig: bool) -> Termios {
        Termios
    }
}

pub fn strerror(e: isize) -> &'static [u8] {
    match e {
        -1 => b"Operation not permitted",
        -2 => b"No such file or directory",
        -9 => b"Bad file descriptor",
        -13 => b"Permission denied",
        -17 => b"File exists",
        -20 => b"Not a directory",
        -21 => b"Is a directory",
        -22 => b"Invalid argument",
        -28 => b"No space left on device",
        -39 => b"Directory not empty",
        _ => b"Error",
    }
}
