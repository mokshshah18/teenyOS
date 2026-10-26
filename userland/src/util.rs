// Small helpers shared by the applets.

use crate::rt::{err, out};
use crate::sys;
use alloc::vec::Vec;

pub fn num(mut n: u64) -> Vec<u8> {
    let mut b = [0u8; 20];
    let mut i = b.len();
    loop {
        i -= 1;
        b[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    b[i..].to_vec()
}

pub fn out_num(n: u64) {
    out(&num(n));
}

/// Right-align `s` in a field `w` wide.
pub fn out_pad(s: &[u8], w: usize) {
    for _ in s.len()..w {
        out(b" ");
    }
    out(s);
}

/// Left-align `s` in a field `w` wide.
pub fn out_padr(s: &[u8], w: usize) {
    out(s);
    for _ in s.len()..w {
        out(b" ");
    }
}

pub fn parse_u64(s: &[u8]) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((c - b'0') as u64)?;
    }
    Some(n)
}

pub fn parse_i64(s: &[u8]) -> Option<i64> {
    match s.first() {
        Some(b'-') => parse_u64(&s[1..]).map(|n| -(n as i64)),
        Some(b'+') => parse_u64(&s[1..]).map(|n| n as i64),
        _ => parse_u64(s).map(|n| n as i64),
    }
}

pub fn out_i64(n: i64) {
    if n < 0 {
        out(b"-");
    }
    out_num(n.unsigned_abs());
}

pub fn parse_octal(s: &[u8]) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let mut n = 0usize;
    for &c in s {
        if !(b'0'..=b'7').contains(&c) {
            return None;
        }
        n = n * 8 + (c - b'0') as usize;
    }
    Some(n)
}

/// Print "prog: what: reason" on stderr.
pub fn perr(prog: &[u8], what: &[u8], e: isize) {
    let mut m = Vec::new();
    m.extend_from_slice(prog);
    m.extend_from_slice(b": ");
    if !what.is_empty() {
        m.extend_from_slice(what);
        m.extend_from_slice(b": ");
    }
    m.extend_from_slice(sys::strerror(e));
    m.push(b'\n');
    err(&m);
}

pub fn msg(prog: &[u8], text: &[u8]) {
    let mut m = Vec::new();
    m.extend_from_slice(prog);
    m.extend_from_slice(b": ");
    m.extend_from_slice(text);
    m.push(b'\n');
    err(&m);
}

pub fn read_fd(fd: i32) -> Vec<u8> {
    let mut v = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sys::read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        v.extend_from_slice(&buf[..n as usize]);
    }
    v
}

pub fn read_file(path: &[u8]) -> Result<Vec<u8>, isize> {
    let fd = sys::open(path, sys::O_RDONLY, 0);
    if fd < 0 {
        return Err(fd as isize);
    }
    let v = read_fd(fd);
    sys::close(fd);
    Ok(v)
}

pub fn write_file(path: &[u8], data: &[u8]) -> Result<(), isize> {
    let fd = sys::open(path, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644);
    if fd < 0 {
        return Err(fd as isize);
    }
    let ok = sys::write_all(fd, data);
    sys::close(fd);
    if ok {
        Ok(())
    } else {
        Err(-5)
    }
}

pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = dir.to_vec();
    if !p.is_empty() && *p.last().unwrap() != b'/' {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

pub fn basename(p: &[u8]) -> &[u8] {
    let mut p = p;
    while p.len() > 1 && p.ends_with(b"/") {
        p = &p[..p.len() - 1];
    }
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) if p.len() > 1 => &p[i + 1..],
        _ => p,
    }
}

pub fn dirname(p: &[u8]) -> &[u8] {
    let mut p = p;
    while p.len() > 1 && p.ends_with(b"/") {
        p = &p[..p.len() - 1];
    }
    match p.iter().rposition(|&c| c == b'/') {
        Some(0) => b"/",
        Some(i) => &p[..i],
        None => b".",
    }
}

/// Entries of a directory, excluding "." and "..", unsorted.
pub fn list_dir(path: &[u8]) -> Result<Vec<Vec<u8>>, isize> {
    let fd = sys::open(path, sys::O_RDONLY | sys::O_DIRECTORY, 0);
    if fd < 0 {
        return Err(fd as isize);
    }
    let mut names = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = sys::getdents(fd, &mut buf);
        if n <= 0 {
            break;
        }
        let mut off = 0usize;
        while off < n as usize {
            let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            let name = unsafe { crate::rt::cstr(buf.as_ptr().add(off + 19)) };
            if name != b"." && name != b".." {
                names.push(name.to_vec());
            }
            off += reclen;
        }
    }
    sys::close(fd);
    Ok(names)
}

/// Split into lines without their terminating newlines.
pub fn lines(data: &[u8]) -> Vec<&[u8]> {
    let mut v: Vec<&[u8]> = data.split(|&c| c == b'\n').collect();
    if data.ends_with(b"\n") || data.is_empty() {
        v.pop();
    }
    v
}

pub fn trim(s: &[u8]) -> &[u8] {
    let a = s
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(s.len());
    let b = s
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(a, |i| i + 1);
    &s[a..b]
}

pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

pub const MONTHS: &[u8] = b"JanFebMarAprMayJunJulAugSepOctNovDec";

pub fn two(n: u64) -> [u8; 2] {
    [b'0' + (n / 10 % 10) as u8, b'0' + (n % 10) as u8]
}

/// "Oct 26 03:35" style timestamp.
pub fn short_time(t: u64) -> Vec<u8> {
    let (_, m, d) = civil((t / 86400) as i64);
    let s = t % 86400;
    let mut v = MONTHS[(m as usize - 1) * 3..m as usize * 3].to_vec();
    v.push(b' ');
    if d < 10 {
        v.push(b' ');
    }
    v.extend_from_slice(&num(d as u64));
    v.push(b' ');
    v.extend_from_slice(&two(s / 3600));
    v.push(b':');
    v.extend_from_slice(&two(s / 60 % 60));
    v
}

/// 1536 -> "1.5K"
pub fn human(n: u64) -> Vec<u8> {
    let units = b"BKMGT";
    let mut u = 0;
    let mut x = n * 10;
    while x >= 10240 && u < 4 {
        x /= 1024;
        u += 1;
    }
    let mut v = num(x / 10);
    if u > 0 && x < 100 {
        v.push(b'.');
        v.push(b'0' + (x % 10) as u8);
    }
    v.push(units[u]);
    v
}
