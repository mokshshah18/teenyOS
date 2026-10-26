// PID 1. Mounts the virtual filesystems, runs /etc/rc, then keeps a login
// shell alive on every console that exists.

use crate::rt::out;
use crate::sys;
use crate::util::*;
use alloc::vec::Vec;

const CONSOLES: &[&[u8]] = &[b"/dev/tty1", b"/dev/ttyS0"];

fn spawn(argv: &[&[u8]], tty: Option<&[u8]>) -> isize {
    let pid = sys::fork();
    if pid != 0 {
        return pid;
    }
    sys::signal(sys::SIGINT, sys::SIG_DFL);
    if let Some(t) = tty {
        sys::setsid();
        let fd = sys::open_inherit(t, sys::O_RDWR, 0);
        if fd < 0 {
            sys::exit(1);
        }
        sys::ioctl(fd, sys::TIOCSCTTY, 1);
        for i in 0..3 {
            sys::dup2(fd, i);
        }
        if fd > 2 {
            sys::close(fd);
        }
    }
    exec(argv);
    sys::exit(127)
}

pub fn exec(argv: &[&[u8]]) -> isize {
    let mut cs: Vec<Vec<u8>> = argv
        .iter()
        .map(|a| {
            let mut v = a.to_vec();
            v.push(0);
            v
        })
        .collect();
    let mut ptrs: Vec<usize> = cs.iter_mut().map(|c| c.as_ptr() as usize).collect();
    ptrs.push(0);
    // A leading '-' marks a login shell; it is not part of the file name.
    let name = argv[0].strip_prefix(b"-").unwrap_or(argv[0]);
    let path = crate::system::which_path(name).unwrap_or_else(|| name.to_vec());
    sys::execve(&path, ptrs.as_ptr(), crate::rt::envp_raw())
}

pub fn main(_: &[&[u8]]) -> i32 {
    if sys::getpid() != 1 {
        msg(b"init", b"must be run as PID 1");
        return 1;
    }
    sys::signal(sys::SIGINT, sys::SIG_IGN);
    sys::mount(b"devtmpfs", b"/dev", b"devtmpfs", 0, b"");
    sys::mount(b"proc", b"/proc", b"proc", 0, b"");
    sys::mount(b"sysfs", b"/sys", b"sysfs", 0, b"");
    sys::mount(b"tmpfs", b"/tmp", b"tmpfs", 0, b"mode=1777");
    sys::mkdir(b"/dev/pts", 0o755);
    sys::mount(b"devpts", b"/dev/pts", b"devpts", 0, b"");
    let cmdline = read_file(b"/proc/cmdline").unwrap_or_default();
    if find(&cmdline, b"teeny.text").is_some() {
        let _ = write_file(b"/run/textmode", b"1");
    }
    if let Ok(h) = read_file(b"/etc/hostname") {
        sys::sethostname(trim(&h));
    }
    if sys::stat(b"/sys/class/net").is_some() {
        spawn(&[b"net", b"daemon"], None);
    }
    crate::persist::boot_scan(&cmdline);
    if sys::stat(b"/etc/rc").is_some() {
        let pid = spawn(&[b"/bin/sh", b"/etc/rc"], None);
        if pid > 0 {
            while sys::wait4(pid, 0).0 > 0 {}
        }
    }

    // One login per console that can actually be opened: the desktop on
    // the screen when there is a framebuffer, a shell everywhere else.
    let mut slots: Vec<(&[u8], isize, u64, bool)> = Vec::new();
    for &c in CONSOLES {
        let fd = sys::open(c, sys::O_RDWR | sys::O_NONBLOCK, 0);
        if fd >= 0 {
            sys::close(fd);
            slots.push((c, 0, 0, false));
        }
    }
    if slots.is_empty() {
        out(b"init: no usable console\n");
        crate::rt::flush();
    }
    loop {
        for s in slots.iter_mut() {
            if s.1 <= 0 {
                // Don't spin if the shell dies instantly.
                if sys::now() == s.2 {
                    sys::sleep_ms(1000);
                }
                s.2 = sys::now();
                s.3 = s.0 == b"/dev/tty1" && sys::stat(b"/dev/fb0").is_some() && sys::stat(b"/run/textmode").is_none();
                s.1 = spawn(if s.3 { &[b"desktop"] } else { &[b"-sh"] }, Some(s.0));
            }
        }
        let (pid, st) = sys::wait4(-1, 0);
        if pid < 0 {
            sys::sleep_ms(1000);
            continue;
        }
        for s in slots.iter_mut() {
            if s.1 == pid {
                s.1 = 0;
                // A desktop that failed to start must not be retried forever.
                if s.3 && st != 0 {
                    let _ = write_file(b"/run/textmode", b"1");
                }
            }
        }
    }
}
