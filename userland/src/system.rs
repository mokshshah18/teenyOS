// System applets: ps kill free df mount umount dmesg uname uptime date
// hostname sleep sync env which whoami clear reboot poweroff halt.

use crate::rt::{self, out};
use crate::sys;
use crate::text::{has, opts};
use crate::util::*;
use alloc::vec::Vec;

pub fn clear(_: &[&[u8]]) -> i32 {
    out(b"\x1b[H\x1b[2J");
    0
}

pub fn sync(_: &[&[u8]]) -> i32 {
    sys::sync();
    0
}

pub fn whoami(_: &[&[u8]]) -> i32 {
    let u = sys::getuid();
    if u == 0 {
        out(b"root");
    } else {
        out_num(u as u64);
    }
    out(b"\n");
    0
}

pub fn env(_: &[&[u8]]) -> i32 {
    for e in rt::env() {
        out(e);
        out(b"\n");
    }
    0
}

pub fn which_path(cmd: &[u8]) -> Option<Vec<u8>> {
    if cmd.contains(&b'/') {
        return Some(cmd.to_vec());
    }
    let path = rt::getenv(b"PATH").unwrap_or(b"/bin:/sbin");
    for dir in path.split(|&c| c == b':') {
        let p = join(dir, cmd);
        if let Some(st) = sys::stat(&p) {
            if !st.is_dir() && st.mode() & 0o111 != 0 {
                return Some(p);
            }
        }
    }
    None
}

pub fn which(args: &[&[u8]]) -> i32 {
    let mut rc = 0;
    for a in &args[1..] {
        match which_path(a) {
            Some(p) => {
                out(&p);
                out(b"\n");
            }
            None => rc = 1,
        }
    }
    rc
}

pub fn sleep(args: &[&[u8]]) -> i32 {
    let Some(a) = args.get(1) else {
        msg(b"sleep", b"usage: sleep SECONDS");
        return 1;
    };
    // Accept "1.5".
    let (int, frac) = match a.iter().position(|&c| c == b'.') {
        Some(i) => (&a[..i], &a[i + 1..]),
        None => (&a[..], &b""[..]),
    };
    let mut ms = parse_u64(if int.is_empty() { b"0" } else { int }).unwrap_or(0) * 1000;
    let mut scale = 100;
    for &c in frac.iter().take(3) {
        ms += (c.wrapping_sub(b'0') as u64 % 10) * scale;
        scale /= 10;
    }
    sys::sleep_ms(ms);
    0
}

pub fn uname(args: &[&[u8]]) -> i32 {
    let (fl, _) = opts(args, b"");
    let mut u = [0u8; 390];
    sys::sc(sys::SYS_UNAME, u.as_mut_ptr() as usize, 0, 0, 0, 0);
    let field = |i: usize| unsafe { rt::cstr(u.as_ptr().add(i * 65)) };
    let all = has(&fl, b'a');
    let mut picks: Vec<usize> = Vec::new();
    for (f, i) in [(b's', 0), (b'n', 1), (b'r', 2), (b'v', 3), (b'm', 4)] {
        if all || has(&fl, f) {
            picks.push(i);
        }
    }
    if picks.is_empty() {
        picks.push(0);
    }
    for (k, i) in picks.iter().enumerate() {
        if k > 0 {
            out(b" ");
        }
        out(field(*i));
    }
    out(b"\n");
    0
}

pub fn hostname(args: &[&[u8]]) -> i32 {
    if let Some(n) = args.get(1) {
        let r = sys::sethostname(n);
        if r < 0 {
            perr(b"hostname", n, r);
            return 1;
        }
        return 0;
    }
    let mut u = [0u8; 390];
    sys::sc(sys::SYS_UNAME, u.as_mut_ptr() as usize, 0, 0, 0, 0);
    out(unsafe { rt::cstr(u.as_ptr().add(65)) });
    out(b"\n");
    0
}

pub fn date(_: &[&[u8]]) -> i32 {
    let t = sys::now();
    let days = (t / 86400) as i64;
    let (y, m, d) = civil(days);
    let wd = ((days + 4).rem_euclid(7)) as usize; // 1970-01-01 was a Thursday
    let s = t % 86400;
    out(&b"SunMonTueWedThuFriSat"[wd * 3..wd * 3 + 3]);
    out(b" ");
    out(&MONTHS[(m as usize - 1) * 3..m as usize * 3]);
    out(b" ");
    out(&two(d as u64));
    out(b" ");
    out(&two(s / 3600));
    out(b":");
    out(&two(s / 60 % 60));
    out(b":");
    out(&two(s % 60));
    out(b" UTC ");
    out_i64(y);
    out(b"\n");
    0
}

fn sysinfo() -> [u32; 16] {
    let mut si = [0u32; 16];
    sys::sc(sys::SYS_SYSINFO, si.as_mut_ptr() as usize, 0, 0, 0, 0);
    si
}

pub fn uptime(_: &[&[u8]]) -> i32 {
    let si = sysinfo();
    let up = si[0] as u64;
    out(b"up ");
    if up >= 86400 {
        out_num(up / 86400);
        out(b" days, ");
    }
    out_num(up / 3600 % 24);
    out(b":");
    out(&two(up / 60 % 60));
    out(b":");
    out(&two(up % 60));
    out(b", ");
    out_num((si[10] & 0xffff) as u64);
    out(b" processes, load average:");
    for i in 1..4 {
        // Loads are fixed point with 16 fractional bits.
        let l = si[i] as u64 * 100 >> 16;
        out(b" ");
        out_num(l / 100);
        out(b".");
        out(&two(l % 100));
    }
    out(b"\n");
    0
}

fn meminfo() -> Vec<(Vec<u8>, u64)> {
    let d = read_file(b"/proc/meminfo").unwrap_or_default();
    let mut v = Vec::new();
    for l in lines(&d) {
        if let Some(i) = l.iter().position(|&c| c == b':') {
            let n = trim(&l[i + 1..]);
            let n = n.split(|&c| c == b' ').next().unwrap_or(b"");
            v.push((l[..i].to_vec(), parse_u64(n).unwrap_or(0)));
        }
    }
    v
}

pub fn free(_: &[&[u8]]) -> i32 {
    let mi = meminfo();
    let get = |k: &[u8]| mi.iter().find(|(n, _)| n == k).map_or(0, |(_, v)| *v);
    let (total, free, avail) = (get(b"MemTotal"), get(b"MemFree"), get(b"MemAvailable"));
    let shared = get(b"Shmem");
    let cache = get(b"Buffers") + get(b"Cached");
    out(b"         total      used      free    shared     cache available\nMem: ");
    for v in [
        total,
        total - free - cache.min(total - free),
        free,
        shared,
        cache,
        avail,
    ] {
        out_pad(&num(v), 9);
        out(b"K");
    }
    out(b"\n");
    0
}

pub fn df(_: &[&[u8]]) -> i32 {
    let mounts = read_file(b"/proc/mounts").unwrap_or_default();
    out(b"Filesystem      Size     Used    Avail  Use% Mounted on\n");
    for l in lines(&mounts) {
        let f: Vec<&[u8]> = l.split(|&c| c == b' ').collect();
        if f.len() < 3 {
            continue;
        }
        let mut st = [0u32; 21];
        let c = sys::CStr::new(f[1]);
        if sys::sc(sys::SYS_STATFS64, c.p(), 84, st.as_mut_ptr() as usize, 0, 0) < 0 {
            continue;
        }
        let bs = st[1] as u64;
        let q = |i: usize| st[i] as u64 | (st[i + 1] as u64) << 32;
        let (blocks, bfree, bavail) = (q(2), q(4), q(6));
        if blocks == 0 {
            continue;
        }
        let used = blocks - bfree;
        out_padr(f[0], 12);
        out_pad(&human(blocks * bs), 8);
        out_pad(&human(used * bs), 9);
        out_pad(&human(bavail * bs), 9);
        out_pad(&num(used * 100 / blocks), 5);
        out(b"% ");
        out(f[1]);
        out(b"\n");
    }
    0
}

pub fn dmesg(_: &[&[u8]]) -> i32 {
    let size = sys::sc(sys::SYS_SYSLOG, 10, 0, 0, 0, 0);
    if size <= 0 {
        return 1;
    }
    let mut buf = alloc::vec![0u8; size as usize];
    let n = sys::sc(
        sys::SYS_SYSLOG,
        3,
        buf.as_mut_ptr() as usize,
        buf.len(),
        0,
        0,
    );
    if n < 0 {
        perr(b"dmesg", b"", n);
        return 1;
    }
    for l in lines(&buf[..n as usize]) {
        // Drop the "<6>" priority prefix.
        let l = if l.first() == Some(&b'<') {
            match l.iter().position(|&c| c == b'>') {
                Some(i) => &l[i + 1..],
                None => l,
            }
        } else {
            l
        };
        out(l);
        out(b"\n");
    }
    0
}

pub fn ps(_: &[&[u8]]) -> i32 {
    let mut pids: Vec<u64> = list_dir(b"/proc")
        .unwrap_or_default()
        .iter()
        .filter_map(|n| parse_u64(n))
        .collect();
    pids.sort();
    out(b"  PID  PPID S   RSS COMMAND\n");
    for pid in pids {
        let dir = join(b"/proc", &num(pid));
        let Ok(stat) = read_file(&join(&dir, b"stat")) else {
            continue;
        };
        // "pid (comm) S ppid ..." where comm may contain spaces or parens.
        let Some(open) = stat.iter().position(|&c| c == b'(') else {
            continue;
        };
        let Some(close) = stat.iter().rposition(|&c| c == b')') else {
            continue;
        };
        let comm = &stat[open + 1..close];
        let rest: Vec<&[u8]> = stat[close + 2..].split(|&c| c == b' ').collect();
        let state = rest.first().copied().unwrap_or(b"?");
        let ppid = rest.get(1).copied().unwrap_or(b"0");
        let rss = rest.get(21).and_then(|r| parse_u64(r)).unwrap_or(0) * 4;
        let mut cmd = read_file(&join(&dir, b"cmdline")).unwrap_or_default();
        while cmd.last() == Some(&0) {
            cmd.pop();
        }
        for c in cmd.iter_mut() {
            if *c == 0 {
                *c = b' ';
            }
        }
        out_pad(&num(pid), 5);
        out_pad(ppid, 6);
        out(b" ");
        out(state);
        out_pad(&num(rss), 6);
        out(b" ");
        if cmd.is_empty() {
            out(b"[");
            out(comm);
            out(b"]");
        } else {
            out(&cmd);
        }
        out(b"\n");
    }
    0
}

const SIGNALS: &[(&[u8], usize)] = &[
    (b"HUP", 1),
    (b"INT", 2),
    (b"QUIT", 3),
    (b"KILL", 9),
    (b"USR1", 10),
    (b"USR2", 12),
    (b"TERM", 15),
    (b"CONT", 18),
    (b"STOP", 19),
];

pub fn kill(args: &[&[u8]]) -> i32 {
    let mut sig = sys::SIGTERM;
    let mut i = 1;
    if let Some(a) = args.get(1) {
        if a == b"-l" {
            for (n, s) in SIGNALS {
                out_pad(&num(*s as u64), 2);
                out(b") SIG");
                out(n);
                out(b"\n");
            }
            return 0;
        }
        if a.len() > 1 && a[0] == b'-' {
            let name = if a.starts_with(b"-SIG") {
                &a[4..]
            } else {
                &a[1..]
            };
            sig = match parse_u64(name) {
                Some(n) => n as usize,
                None => match SIGNALS.iter().find(|(n, _)| *n == name) {
                    Some((_, s)) => *s,
                    None => {
                        msg(b"kill", b"unknown signal");
                        return 1;
                    }
                },
            };
            i = 2;
        }
    }
    if i >= args.len() {
        msg(b"kill", b"usage: kill [-SIGNAL] PID...");
        return 1;
    }
    let mut rc = 0;
    for a in &args[i..] {
        let Some(pid) = parse_i64(a) else {
            msg(b"kill", b"bad pid");
            rc = 1;
            continue;
        };
        let r = sys::kill(pid as isize, sig);
        if r < 0 {
            perr(b"kill", a, r);
            rc = 1;
        }
    }
    rc
}

pub fn mount(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"to");
    if ops.is_empty() {
        let m = read_file(b"/proc/mounts").unwrap_or_default();
        out(&m);
        return 0;
    }
    if ops.len() != 2 {
        msg(b"mount", b"usage: mount [-t TYPE] [-o OPTS] SOURCE DIR");
        return 1;
    }
    let fstype = crate::text::val(&fl, b't').unwrap_or(ops[0]);
    let mut flags = 0usize;
    let mut data = Vec::new();
    for o in crate::text::val(&fl, b'o')
        .unwrap_or(b"")
        .split(|&c| c == b',')
    {
        match o {
            b"" => {}
            b"ro" => flags |= 1,
            b"rw" => {}
            b"remount" => flags |= 32,
            b"bind" => flags |= 4096,
            _ => {
                if !data.is_empty() {
                    data.push(b',');
                }
                data.extend_from_slice(o);
            }
        }
    }
    let r = sys::mount(ops[0], ops[1], fstype, flags, &data);
    if r < 0 {
        perr(b"mount", ops[1], r);
        return 1;
    }
    0
}

pub fn umount(args: &[&[u8]]) -> i32 {
    let mut rc = 0;
    for p in &args[1..] {
        let r = sys::umount(p);
        if r < 0 {
            perr(b"umount", p, r);
            rc = 1;
        }
    }
    rc
}

fn shutdown(cmd: usize, what: &[u8]) -> i32 {
    // Killing everything takes out our parent shell, and the kernel then
    // sends SIGHUP to its foreground group, which includes us.
    sys::signal(sys::SIGHUP, sys::SIG_IGN);
    sys::signal(sys::SIGTERM, sys::SIG_IGN);
    out(b"\nSystem is going down for ");
    out(what);
    out(b".\n");
    rt::flush();
    sys::sync();
    sys::kill(-1, sys::SIGTERM);
    sys::sleep_ms(300);
    sys::kill(-1, sys::SIGKILL);
    sys::sync();
    let r = sys::reboot(cmd);
    perr(what, b"", r);
    1
}

pub fn reboot(_: &[&[u8]]) -> i32 {
    shutdown(0x01234567, b"reboot")
}

pub fn poweroff(_: &[&[u8]]) -> i32 {
    shutdown(0x4321fedc, b"poweroff")
}

pub fn halt(_: &[&[u8]]) -> i32 {
    shutdown(0xcdef0123, b"halt")
}
