// File applets: ls cp mv rm mkdir rmdir ln touch pwd chmod stat.

use crate::rt::out;
use crate::sys;
use crate::text::{has, opts};
use crate::util::*;
use alloc::vec::Vec;

fn mode_string(m: u32) -> [u8; 10] {
    let t = match m & sys::S_IFMT {
        sys::S_IFDIR => b'd',
        sys::S_IFLNK => b'l',
        sys::S_IFCHR => b'c',
        sys::S_IFBLK => b'b',
        0o010000 => b'p',
        0o140000 => b's',
        _ => b'-',
    };
    let mut s = [t, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let rwx = b"rwx";
    for i in 0..9 {
        s[i + 1] = if m & (0o400 >> i) != 0 {
            rwx[i % 3]
        } else {
            b'-'
        };
    }
    s
}

fn color_for(st: &sys::Stat) -> &'static [u8] {
    match st.mode() & sys::S_IFMT {
        sys::S_IFDIR => b"\x1b[1;34m",
        sys::S_IFLNK => b"\x1b[1;36m",
        sys::S_IFCHR | sys::S_IFBLK => b"\x1b[1;33m",
        _ if st.mode() & 0o111 != 0 => b"\x1b[1;32m",
        _ => b"",
    }
}

fn ls_long(path: &[u8], name: &[u8], st: &sys::Stat, color: bool) {
    out(&mode_string(st.mode()));
    out_pad(&num(st.nlink() as u64), 3);
    out(b" ");
    out_padr(
        &if st.uid() == 0 {
            b"root".to_vec()
        } else {
            num(st.uid() as u64)
        },
        5,
    );
    let m = st.mode() & sys::S_IFMT;
    if m == sys::S_IFCHR || m == sys::S_IFBLK {
        let r = st.rdev();
        out_pad(&num(((r >> 8) & 0xfff) as u64), 4);
        out(b",");
        out_pad(&num((r & 0xff) as u64), 4);
    } else {
        out_pad(&num(st.size()), 9);
    }
    out(b" ");
    out(&short_time(st.mtime()));
    out(b" ");
    ls_name(name, st, color);
    if st.is_lnk() {
        let mut buf = [0u8; 256];
        let n = sys::readlink(path, &mut buf);
        if n > 0 {
            out(b" -> ");
            out(&buf[..n as usize]);
        }
    }
    out(b"\n");
}

fn ls_name(name: &[u8], st: &sys::Stat, color: bool) {
    let c = if color { color_for(st) } else { b"" };
    out(c);
    out(name);
    if !c.is_empty() {
        out(b"\x1b[0m");
    }
}

pub fn ls(args: &[&[u8]]) -> i32 {
    let (fl, mut ops) = opts(args, b"");
    let (long, all, one) = (has(&fl, b'l'), has(&fl, b'a'), has(&fl, b'1'));
    let tty = sys::isatty(1);
    let color = tty;
    if ops.is_empty() {
        ops.push(b".");
    }
    let mut rc = 0;
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    for p in &ops {
        match sys::lstat(p) {
            None => {
                perr(b"ls", p, -sys::ENOENT);
                rc = 1;
            }
            Some(st) => {
                let is_dir = st.is_dir()
                    || (st.is_lnk() && sys::stat(p).map_or(false, |s| s.is_dir()) && !long);
                if is_dir {
                    dirs.push(*p);
                } else {
                    files.push((p.to_vec(), p.to_vec(), st));
                }
            }
        }
    }
    let show = |entries: &mut Vec<(Vec<u8>, Vec<u8>, sys::Stat)>| {
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        if long {
            for (path, name, st) in entries.iter() {
                ls_long(path, name, st, color);
            }
        } else if one || !tty {
            for (_, name, st) in entries.iter() {
                ls_name(name, st, color);
                out(b"\n");
            }
        } else {
            let (_, cols) = sys::winsize(1);
            let w = entries.iter().map(|e| e.1.len()).max().unwrap_or(0) + 2;
            let per = (cols / w).max(1);
            let rows = entries.len().div_ceil(per);
            for r in 0..rows {
                let mut k = r;
                while k < entries.len() {
                    let (_, name, st) = &entries[k];
                    ls_name(name, st, color);
                    if k + rows < entries.len() {
                        for _ in name.len()..w {
                            out(b" ");
                        }
                    }
                    k += rows;
                }
                out(b"\n");
            }
        }
    };
    show(&mut files);
    let multi = ops.len() > 1;
    for (i, d) in dirs.iter().enumerate() {
        if multi {
            if i > 0 || !files.is_empty() {
                out(b"\n");
            }
            out(d);
            out(b":\n");
        }
        let names = match list_dir(d) {
            Ok(n) => n,
            Err(e) => {
                perr(b"ls", d, e);
                rc = 1;
                continue;
            }
        };
        let mut entries = Vec::new();
        for n in names {
            if !all && n.first() == Some(&b'.') {
                continue;
            }
            let p = join(d, &n);
            if let Some(st) = sys::lstat(&p) {
                entries.push((p, n, st));
            }
        }
        show(&mut entries);
    }
    rc
}

pub fn pwd(_: &[&[u8]]) -> i32 {
    let mut buf = [0u8; 1024];
    let n = sys::getcwd(&mut buf);
    if n <= 0 {
        return 1;
    }
    out(&buf[..n as usize - 1]);
    out(b"\n");
    0
}

pub fn mkdir(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let parents = has(&fl, b'p');
    let mut rc = 0;
    for p in ops {
        if parents {
            for (i, &c) in p.iter().enumerate() {
                if c == b'/' && i > 0 {
                    sys::mkdir(&p[..i], 0o755);
                }
            }
        }
        let r = sys::mkdir(p, 0o755);
        if r < 0 && !(parents && r == -sys::EEXIST) {
            perr(b"mkdir", p, r);
            rc = 1;
        }
    }
    rc
}

pub fn rmdir(args: &[&[u8]]) -> i32 {
    let mut rc = 0;
    for p in &args[1..] {
        let r = sys::rmdir(p);
        if r < 0 {
            perr(b"rmdir", p, r);
            rc = 1;
        }
    }
    rc
}

fn remove(p: &[u8], recursive: bool) -> isize {
    match sys::lstat(p) {
        None => -sys::ENOENT,
        Some(st) if st.is_dir() => {
            if !recursive {
                return -sys::EISDIR;
            }
            if let Ok(names) = list_dir(p) {
                for n in names {
                    let r = remove(&join(p, &n), true);
                    if r < 0 {
                        return r;
                    }
                }
            }
            sys::rmdir(p)
        }
        Some(_) => sys::unlink(p),
    }
}

pub fn rm(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let (rec, force) = (has(&fl, b'r') || has(&fl, b'R'), has(&fl, b'f'));
    let mut rc = 0;
    for p in ops {
        let r = remove(p, rec);
        if r < 0 && !(force && r == -sys::ENOENT) {
            perr(b"rm", p, r);
            rc = 1;
        }
    }
    rc
}

fn copy_file(src: &[u8], dst: &[u8], mode: u32) -> isize {
    let i = sys::open(src, sys::O_RDONLY, 0);
    if i < 0 {
        return i as isize;
    }
    let o = sys::open(
        dst,
        sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC,
        (mode & 0o7777) as usize,
    );
    if o < 0 {
        sys::close(i);
        return o as isize;
    }
    let mut buf = [0u8; 4096];
    let mut r = 0;
    loop {
        let n = sys::read(i, &mut buf);
        if n < 0 {
            r = n;
        }
        if n <= 0 {
            break;
        }
        if !sys::write_all(o, &buf[..n as usize]) {
            r = -28;
            break;
        }
    }
    sys::close(i);
    sys::close(o);
    r
}

fn copy_tree(src: &[u8], dst: &[u8], recursive: bool) -> isize {
    let Some(st) = sys::lstat(src) else {
        return -sys::ENOENT;
    };
    if st.is_lnk() {
        let mut buf = [0u8; 1024];
        let n = sys::readlink(src, &mut buf);
        if n < 0 {
            return n;
        }
        return sys::symlink(&buf[..n as usize], dst);
    }
    if st.is_dir() {
        if !recursive {
            return -sys::EISDIR;
        }
        let r = sys::mkdir(dst, (st.mode() & 0o7777) as usize);
        if r < 0 && r != -sys::EEXIST {
            return r;
        }
        for n in list_dir(src).unwrap_or_default() {
            let r = copy_tree(&join(src, &n), &join(dst, &n), true);
            if r < 0 {
                return r;
            }
        }
        return 0;
    }
    copy_file(src, dst, st.mode())
}

/// Resolve the destination for "cp/mv SRC... DST" when DST is a directory.
fn target(src: &[u8], dst: &[u8], many: bool) -> Vec<u8> {
    if many || sys::stat(dst).map_or(false, |s| s.is_dir()) {
        join(dst, basename(src))
    } else {
        dst.to_vec()
    }
}

pub fn cp(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let rec = has(&fl, b'r') || has(&fl, b'R') || has(&fl, b'a');
    if ops.len() < 2 {
        msg(b"cp", b"usage: cp [-r] SRC... DST");
        return 1;
    }
    let (srcs, dst) = ops.split_at(ops.len() - 1);
    let mut rc = 0;
    for s in srcs {
        let t = target(s, dst[0], srcs.len() > 1);
        let r = copy_tree(s, &t, rec);
        if r < 0 {
            perr(b"cp", s, r);
            rc = 1;
        }
    }
    rc
}

pub fn mv(args: &[&[u8]]) -> i32 {
    let (_, ops) = opts(args, b"");
    if ops.len() < 2 {
        msg(b"mv", b"usage: mv SRC... DST");
        return 1;
    }
    let (srcs, dst) = ops.split_at(ops.len() - 1);
    let mut rc = 0;
    for s in srcs {
        let t = target(s, dst[0], srcs.len() > 1);
        let mut r = sys::rename(s, &t);
        if r == -sys::EXDEV {
            // Moving between the root ramfs and a tmpfs: copy, then delete.
            r = copy_tree(s, &t, true);
            if r >= 0 {
                r = remove(s, true);
            }
        }
        if r < 0 {
            perr(b"mv", s, r);
            rc = 1;
        }
    }
    rc
}

pub fn ln(args: &[&[u8]]) -> i32 {
    let (fl, ops) = opts(args, b"");
    let (soft, force) = (has(&fl, b's'), has(&fl, b'f'));
    if ops.is_empty() {
        msg(b"ln", b"usage: ln [-sf] TARGET [LINK]");
        return 1;
    }
    let src = ops[0];
    let dst = if ops.len() > 1 {
        target(src, ops[1], false)
    } else {
        basename(src).to_vec()
    };
    if force {
        sys::unlink(&dst);
    }
    let r = if soft {
        sys::symlink(src, &dst)
    } else {
        sys::link(src, &dst)
    };
    if r < 0 {
        perr(b"ln", &dst, r);
        return 1;
    }
    0
}

pub fn touch(args: &[&[u8]]) -> i32 {
    let mut rc = 0;
    for p in &args[1..] {
        let fd = sys::open(p, sys::O_WRONLY | sys::O_CREAT, 0o644);
        if fd < 0 {
            if sys::touch_now(p) < 0 {
                perr(b"touch", p, fd as isize);
                rc = 1;
            }
            continue;
        }
        sys::close(fd);
        sys::touch_now(p);
    }
    rc
}

pub fn chmod(args: &[&[u8]]) -> i32 {
    if args.len() < 3 {
        msg(b"chmod", b"usage: chmod MODE FILE...");
        return 1;
    }
    let mut rc = 0;
    for p in &args[2..] {
        let Some(st) = sys::stat(p) else {
            perr(b"chmod", p, -sys::ENOENT);
            rc = 1;
            continue;
        };
        let Some(m) = symbolic_mode(args[1], st.mode() & 0o7777) else {
            msg(b"chmod", b"bad mode");
            return 1;
        };
        let r = sys::chmod(p, m as usize);
        if r < 0 {
            perr(b"chmod", p, r);
            rc = 1;
        }
    }
    rc
}

/// Octal ("755") or simple symbolic ("+x", "u+rw", "go-w") modes.
fn symbolic_mode(s: &[u8], cur: u32) -> Option<u32> {
    if let Some(m) = parse_octal(s) {
        return Some(m as u32);
    }
    let op_at = s.iter().position(|c| b"+-=".contains(c))?;
    let who = &s[..op_at];
    let mut mask = 0u32;
    for &w in if who.is_empty() { b"a" as &[u8] } else { who } {
        mask |= match w {
            b'u' => 0o700,
            b'g' => 0o070,
            b'o' => 0o007,
            b'a' => 0o777,
            _ => return None,
        };
    }
    let mut bits = 0u32;
    for &p in &s[op_at + 1..] {
        bits |= match p {
            b'r' => 0o444,
            b'w' => 0o222,
            b'x' => 0o111,
            _ => return None,
        };
    }
    bits &= mask;
    Some(match s[op_at] {
        b'+' => cur | bits,
        b'-' => cur & !bits,
        _ => (cur & !mask) | bits,
    })
}

pub fn stat(args: &[&[u8]]) -> i32 {
    let mut rc = 0;
    for p in &args[1..] {
        let Some(st) = sys::lstat(p) else {
            perr(b"stat", p, -sys::ENOENT);
            rc = 1;
            continue;
        };
        out(b"  File: ");
        out(p);
        out(b"\n  Size: ");
        out_num(st.size());
        out(b"\t Inode: ");
        out_num(st.ino());
        out(b"\t Links: ");
        out_num(st.nlink() as u64);
        out(b"\n  Mode: ");
        out(&mode_string(st.mode()));
        out(b"\t Uid: ");
        out_num(st.uid() as u64);
        out(b"\t Gid: ");
        out_num(st.gid() as u64);
        out(b"\t Device: ");
        out_num(st.dev());
        out(b"\nModify: ");
        out(&short_time(st.mtime()));
        out(b"\n");
    }
    rc
}
