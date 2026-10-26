// Text applets: cat echo wc head tail grep sort uniq tee seq more hexdump
// basename dirname test true false.

use crate::rt::{flush, out};
use crate::sysb as sys;
use crate::util::*;
use alloc::vec::Vec;

/// Split argv into (flags, operands). Flags are single letters, possibly
/// bundled ("-ln"). A lone "-" or anything after "--" is an operand.
pub fn opts<'a>(args: &[&'a [u8]], with_value: &[u8]) -> (Vec<(u8, &'a [u8])>, Vec<&'a [u8]>) {
    let mut flags = Vec::new();
    let mut ops = Vec::new();
    let mut i = 1;
    let mut done = false;
    while i < args.len() {
        let a = args[i];
        if !done && a == b"--" {
            done = true;
        } else if !done && a.len() > 1 && a[0] == b'-' && !a[1].is_ascii_digit() {
            let mut j = 1;
            while j < a.len() {
                let f = a[j];
                if with_value.contains(&f) {
                    let v: &[u8] = if j + 1 < a.len() {
                        &a[j + 1..]
                    } else {
                        i += 1;
                        args.get(i).copied().unwrap_or(b"")
                    };
                    flags.push((f, v));
                    break;
                }
                flags.push((f, b"" as &[u8]));
                j += 1;
            }
        } else {
            ops.push(a);
        }
        i += 1;
    }
    (flags, ops)
}

pub fn has(flags: &[(u8, &[u8])], f: u8) -> bool {
    flags.iter().any(|&(c, _)| c == f)
}

pub fn val<'a>(flags: &[(u8, &'a [u8])], f: u8) -> Option<&'a [u8]> {
    flags.iter().rev().find(|&&(c, _)| c == f).map(|&(_, v)| v)
}

/// Read every operand (or stdin when there are none) and hand each to `f`.
fn each_input(prog: &[u8], ops: &[&[u8]], mut f: impl FnMut(&[u8], &[u8])) -> i32 {
    let mut rc = 0;
    if ops.is_empty() {
        f(b"-", &read_fd(0));
    }
    for &p in ops {
        if p == b"-" {
            f(p, &read_fd(0));
            continue;
        }
        match read_file(p) {
            Ok(d) => f(p, &d),
            Err(e) => {
                perr(prog, p, e);
                rc = 1;
            }
        }
    }
    rc
}

pub fn cat(args: &[&[u8]]) -> i32 {
    let (_, ops) = opts(args, b"");
    let mut rc = 0;
    let stream = |fd: i32| {
        flush();
        let mut buf = [0u8; 4096];
        loop {
            let n = sys::read(fd, &mut buf);
            if n <= 0 || !sys::write_all(1, &buf[..n as usize]) {
                break;
            }
        }
    };
    if ops.is_empty() {
        stream(0);
    }
    for p in ops {
        if p == b"-" {
            stream(0);
            continue;
        }
        let fd = sys::open(p, sys::O_RDONLY, 0);
        if fd < 0 {
            perr(b"cat", p, fd as isize);
            rc = 1;
            continue;
        }
        stream(fd);
        sys::close(fd);
    }
    rc
}

pub fn echo(args: &[&[u8]]) -> i32 {
    let mut i = 1;
    let (mut nl, mut esc) = (true, false);
    while i < args.len()
        && args[i].len() > 1
        && args[i][0] == b'-'
        && args[i][1..].iter().all(|c| b"ne".contains(c))
    {
        nl &= !args[i].contains(&b'n');
        esc |= args[i].contains(&b'e');
        i += 1;
    }
    for (k, a) in args[i..].iter().enumerate() {
        if k > 0 {
            out(b" ");
        }
        if !esc {
            out(a);
            continue;
        }
        let mut j = 0;
        while j < a.len() {
            if a[j] == b'\\' && j + 1 < a.len() {
                j += 1;
                let c = match a[j] {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    b'e' => 27,
                    b'\\' => b'\\',
                    b'c' => return 0,
                    o => {
                        out(b"\\");
                        o
                    }
                };
                out(&[c]);
            } else {
                out(&a[j..j + 1]);
            }
            j += 1;
        }
    }
    if nl {
        out(b"\n");
    }
    0
}

pub fn wc(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let all = !has(&fl, b'l') && !has(&fl, b'w') && !has(&fl, b'c');
    let mut tot = [0u64; 3];
    let n = ops.len();
    let rc = each_input(b"wc", &ops, |name, d| {
        let l = d.iter().filter(|&&c| c == b'\n').count() as u64;
        let w = d
            .split(|c| c.is_ascii_whitespace())
            .filter(|s| !s.is_empty())
            .count() as u64;
        let c = d.len() as u64;
        let v = [l, w, c];
        for k in 0..3 {
            tot[k] += v[k];
        }
        show_wc(&v, &fl, all, if name == b"-" { b"" } else { name });
    });
    if n > 1 {
        show_wc(&tot, &fl, all, b"total");
    }
    rc
}

fn show_wc(v: &[u64; 3], fl: &[(u8, &[u8])], all: bool, name: &[u8]) {
    for (k, f) in [b'l', b'w', b'c'].iter().enumerate() {
        if all || has(fl, *f) {
            out_pad(&num(v[k]), 7);
            out(b" ");
        }
    }
    out(name);
    out(b"\n");
}

fn count_arg(fl: &[(u8, &[u8])], args: &[&[u8]]) -> usize {
    // Accept both "-n 5" and the old "-5".
    if let Some(v) = val(fl, b'n') {
        return parse_u64(v).unwrap_or(10) as usize;
    }
    for a in &args[1..] {
        if a.len() > 1 && a[0] == b'-' {
            if let Some(n) = parse_u64(&a[1..]) {
                return n as usize;
            }
        }
    }
    10
}

fn strip_count<'a>(ops: Vec<&'a [u8]>) -> Vec<&'a [u8]> {
    ops.into_iter()
        .filter(|a| !(a.len() > 1 && a[0] == b'-' && parse_u64(&a[1..]).is_some()))
        .collect()
}

pub fn head(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"n");
    let n = count_arg(&fl, args);
    let ops = strip_count(ops);
    each_input(b"head", &ops, |_, d| {
        for l in lines(d).iter().take(n) {
            out(l);
            out(b"\n");
        }
    })
}

pub fn tail(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"n");
    let n = count_arg(&fl, args);
    let ops = strip_count(ops);
    each_input(b"tail", &ops, |_, d| {
        let ls = lines(d);
        for l in &ls[ls.len().saturating_sub(n)..] {
            out(l);
            out(b"\n");
        }
    })
}

// Pike/Kernighan matcher: literal chars, '.', '*', '^' and '$'.
fn re_match(re: &[u8], text: &[u8], icase: bool) -> bool {
    if re.first() == Some(&b'^') {
        return re_here(&re[1..], text, icase);
    }
    let mut t = text;
    loop {
        if re_here(re, t, icase) {
            return true;
        }
        if t.is_empty() {
            return false;
        }
        t = &t[1..];
    }
}

fn re_eq(p: u8, c: u8, icase: bool) -> bool {
    p == b'.' || p == c || (icase && p.eq_ignore_ascii_case(&c))
}

fn re_here(re: &[u8], text: &[u8], icase: bool) -> bool {
    if re.is_empty() {
        return true;
    }
    if re.len() >= 2 && re[1] == b'*' {
        let mut t = text;
        loop {
            if re_here(&re[2..], t, icase) {
                return true;
            }
            if t.is_empty() || !re_eq(re[0], t[0], icase) {
                return false;
            }
            t = &t[1..];
        }
    }
    if re == b"$" {
        return text.is_empty();
    }
    if !text.is_empty() && re_eq(re[0], text[0], icase) {
        return re_here(&re[1..], &text[1..], icase);
    }
    false
}

pub fn grep(args: &[&[u8]]) -> i32 {
    let (fl, mut ops) = opts(args, b"e");
    let pat: &[u8] = match val(&fl, b'e') {
        Some(p) => p,
        None if !ops.is_empty() => ops.remove(0),
        None => {
            msg(b"grep", b"usage: grep [-ivncE] PATTERN [FILE...]");
            return 2;
        }
    };
    let (icase, inv, numb, cnt) = (
        has(&fl, b'i'),
        has(&fl, b'v'),
        has(&fl, b'n'),
        has(&fl, b'c'),
    );
    // Alternation: "a\|b", or "a|b" with -E.
    let alts: Vec<Vec<u8>> = if has(&fl, b'E') {
        pat.split(|&c| c == b'|').map(|p| p.to_vec()).collect()
    } else {
        let mut v = Vec::new();
        let mut rest = pat;
        while let Some(i) = find(rest, b"\\|") {
            v.push(rest[..i].to_vec());
            rest = &rest[i + 2..];
        }
        v.push(rest.to_vec());
        v
    };
    let multi = ops.len() > 1;
    let mut found = false;
    let rc = each_input(b"grep", &ops, |name, d| {
        let mut c = 0u64;
        for (i, l) in lines(d).iter().enumerate() {
            if alts.iter().any(|p| re_match(p, l, icase)) != inv {
                found = true;
                c += 1;
                if cnt {
                    continue;
                }
                if multi {
                    out(name);
                    out(b":");
                }
                if numb {
                    out_num(i as u64 + 1);
                    out(b":");
                }
                out(l);
                out(b"\n");
            }
        }
        if cnt {
            if multi {
                out(name);
                out(b":");
            }
            out_num(c);
            out(b"\n");
        }
    });
    if rc != 0 {
        2
    } else if found {
        0
    } else {
        1
    }
}

pub fn sort(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let mut all: Vec<Vec<u8>> = Vec::new();
    let rc = each_input(b"sort", &ops, |_, d| {
        all.extend(lines(d).iter().map(|l| l.to_vec()))
    });
    if has(&fl, b'n') {
        all.sort_by_key(|l| parse_i64(trim(l)).unwrap_or(0));
    } else {
        all.sort();
    }
    if has(&fl, b'r') {
        all.reverse();
    }
    if has(&fl, b'u') {
        all.dedup();
    }
    for l in all {
        out(&l);
        out(b"\n");
    }
    rc
}

pub fn uniq(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let count = has(&fl, b'c');
    each_input(b"uniq", &ops, |_, d| {
        let ls = lines(d);
        let mut i = 0;
        while i < ls.len() {
            let mut j = i + 1;
            while j < ls.len() && ls[j] == ls[i] {
                j += 1;
            }
            if count {
                out_pad(&num((j - i) as u64), 7);
                out(b" ");
            }
            out(ls[i]);
            out(b"\n");
            i = j;
        }
    })
}

pub fn tee(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let mode = if has(&fl, b'a') {
        sys::O_APPEND
    } else {
        sys::O_TRUNC
    };
    let mut fds = Vec::new();
    let mut rc = 0;
    for p in &ops {
        let fd = sys::open(p, sys::O_WRONLY | sys::O_CREAT | mode, 0o644);
        if fd < 0 {
            perr(b"tee", p, fd as isize);
            rc = 1;
        } else {
            fds.push(fd);
        }
    }
    let mut buf = [0u8; 4096];
    loop {
        let n = sys::read(0, &mut buf);
        if n <= 0 {
            break;
        }
        sys::write_all(1, &buf[..n as usize]);
        for &fd in &fds {
            sys::write_all(fd, &buf[..n as usize]);
        }
    }
    rc
}

pub fn seq(args: &[&[u8]]) -> i32 {
    let n: Vec<i64> = args[1..].iter().filter_map(|a| parse_i64(a)).collect();
    let (a, step, b) = match n.len() {
        1 => (1, 1, n[0]),
        2 => (n[0], 1, n[1]),
        3 => (n[0], n[1], n[2]),
        _ => {
            msg(b"seq", b"usage: seq [FIRST [STEP]] LAST");
            return 1;
        }
    };
    if step == 0 {
        return 1;
    }
    let mut i = a;
    while (step > 0 && i <= b) || (step < 0 && i >= b) {
        out_i64(i);
        out(b"\n");
        i += step;
    }
    0
}

pub fn basename(args: &[&[u8]]) -> i32 {
    if args.len() < 2 {
        return 1;
    }
    let mut b = crate::util::basename(args[1]);
    if let Some(suf) = args.get(2) {
        if b.len() > suf.len() && b.ends_with(suf) {
            b = &b[..b.len() - suf.len()];
        }
    }
    out(b);
    out(b"\n");
    0
}

pub fn dirname(args: &[&[u8]]) -> i32 {
    if args.len() < 2 {
        return 1;
    }
    out(crate::util::dirname(args[1]));
    out(b"\n");
    0
}

pub fn true_(_: &[&[u8]]) -> i32 {
    0
}

pub fn false_(_: &[&[u8]]) -> i32 {
    1
}

pub fn test(args: &[&[u8]]) -> i32 {
    let mut a: Vec<&[u8]> = args[1..].to_vec();
    if crate::util::basename(args[0]) == b"[" {
        if a.last() != Some(&&b"]"[..]) {
            msg(b"[", b"missing ]");
            return 2;
        }
        a.pop();
    }
    let mut neg = false;
    while a.first() == Some(&&b"!"[..]) && a.len() > 1 {
        neg = !neg;
        a.remove(0);
    }
    let r = match a.len() {
        0 => false,
        1 => !a[0].is_empty(),
        2 => {
            let p = a[1];
            match a[0] {
                b"-n" => !p.is_empty(),
                b"-z" => p.is_empty(),
                b"-e" => sys::stat(p).is_some(),
                b"-f" => sys::stat(p).map_or(false, |s| s.mode() & sys::S_IFMT == sys::S_IFREG),
                b"-d" => sys::stat(p).map_or(false, |s| s.is_dir()),
                b"-L" | b"-h" => sys::lstat(p).map_or(false, |s| s.is_lnk()),
                b"-s" => sys::stat(p).map_or(false, |s| s.size() > 0),
                b"-x" => sys::stat(p).map_or(false, |s| s.mode() & 0o111 != 0),
                b"-r" | b"-w" => sys::stat(p).is_some(),
                _ => {
                    msg(b"test", b"unknown operator");
                    return 2;
                }
            }
        }
        3 => {
            let (x, op, y) = (a[0], a[1], a[2]);
            match op {
                b"=" | b"==" => x == y,
                b"!=" => x != y,
                _ => {
                    let (Some(i), Some(j)) = (parse_i64(x), parse_i64(y)) else {
                        msg(b"test", b"integer expected");
                        return 2;
                    };
                    match op {
                        b"-eq" => i == j,
                        b"-ne" => i != j,
                        b"-lt" => i < j,
                        b"-le" => i <= j,
                        b"-gt" => i > j,
                        b"-ge" => i >= j,
                        _ => {
                            msg(b"test", b"unknown operator");
                            return 2;
                        }
                    }
                }
            }
        }
        _ => {
            msg(b"test", b"too many arguments");
            return 2;
        }
    };
    (r == neg) as i32
}

pub fn hexdump(args: &[&[u8]]) -> i32 {
    let (_, ops) = opts(args, b"");
    let hex = b"0123456789abcdef";
    each_input(b"hexdump", &ops, |_, d| {
        for (row, chunk) in d.chunks(16).enumerate() {
            let off = row * 16;
            for s in [28, 24, 20, 16, 12, 8, 4, 0] {
                out(&[hex[(off >> s) & 15]]);
            }
            out(b"  ");
            for i in 0..16 {
                match chunk.get(i) {
                    Some(&b) => out(&[hex[(b >> 4) as usize], hex[(b & 15) as usize], b' ']),
                    None => out(b"   "),
                }
                if i == 7 {
                    out(b" ");
                }
            }
            out(b" |");
            for &b in chunk {
                out(&[if (32..127).contains(&b) { b } else { b'.' }]);
            }
            out(b"|\n");
        }
    })
}

/// Pager. Reads keys from /dev/tty so it works at the end of a pipe.
pub fn more(args: &[&[u8]]) -> i32 {
    let (_, ops) = opts(args, b"");
    let mut all = Vec::new();
    let rc = each_input(b"more", &ops, |_, d| all.extend_from_slice(d));
    let tty = sys::open(b"/dev/tty", sys::O_RDONLY, 0);
    let saved = if tty >= 0 {
        sys::Termios::get(tty)
    } else {
        None
    };
    let Some(saved) = saved else {
        out(&all);
        return rc;
    };
    saved.raw(true).set(tty);
    let (rows, _) = sys::winsize(1);
    let ls = lines(&all);
    let mut shown = 0;
    let mut budget = rows - 1;
    while shown < ls.len() {
        while budget > 0 && shown < ls.len() {
            out(ls[shown]);
            out(b"\n");
            shown += 1;
            budget -= 1;
        }
        if shown >= ls.len() {
            break;
        }
        out(b"\x1b[7m--More-- ");
        out_num((shown * 100 / ls.len()) as u64);
        out(b"%\x1b[0m");
        flush();
        let mut k = [0u8; 1];
        if sys::read(tty, &mut k) <= 0 {
            break;
        }
        out(b"\r\x1b[K");
        match k[0] {
            b'q' | b'Q' | 3 => break,
            b'\r' | b'\n' | b'j' => budget = 1,
            _ => budget = rows - 1,
        }
    }
    flush();
    saved.set(tty);
    sys::close(tty);
    rc
}
