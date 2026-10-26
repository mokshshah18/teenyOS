// An in-RAM filesystem: the whole tree lives in memory, like the Linux
// edition's ramfs. Persistence (persist.rs) saves and restores a subtree as a
// single archive file on the firmware's FAT volume, so teenyOS core needs no
// filesystem driver of its own.

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFMT: u32 = 0o170000;

pub const ENOENT: i32 = -2;
pub const EEXIST: i32 = -17;
pub const ENOTDIR: i32 = -20;
pub const EISDIR: i32 = -21;
pub const EINVAL: i32 = -22;
pub const ENOTEMPTY: i32 = -39;

enum Body {
    Dir(Vec<(Vec<u8>, usize)>), // (name, node index)
    File(Vec<u8>),
    Link(Vec<u8>),
}

struct Node {
    body: Body,
    mode: u32, // permission bits only
    mtime: u64,
    nlink: u32,
}

pub struct Stat {
    pub mode: u32,
    pub size: u64,
    pub mtime: u64,
    pub ino: u64,
    pub nlink: u32,
}

struct Fs {
    nodes: Vec<Node>,
    now: u64,
}

struct SyncCell(RefCell<Option<Fs>>);
unsafe impl Sync for SyncCell {}
static FS: SyncCell = SyncCell(RefCell::new(None));

fn with<R>(f: impl FnOnce(&mut Fs) -> R) -> R {
    let mut b = FS.0.borrow_mut();
    f(b.as_mut().expect("vfs not initialised"))
}

/// Set the clock source for mtimes (seconds since epoch).
pub fn set_now(t: u64) {
    with(|fs| fs.now = t);
}

pub fn init() {
    let root = Node { body: Body::Dir(Vec::new()), mode: 0o755, mtime: 0, nlink: 2 };
    *FS.0.borrow_mut() = Some(Fs { nodes: vec![root], now: 0 });
    for d in [b"/bin" as &[u8], b"/dev", b"/tmp", b"/etc", b"/root", b"/home", b"/mnt", b"/run"] {
        let _ = mkdir(d, 0o755);
    }
}

impl Fs {
    fn child(&self, dir: usize, name: &[u8]) -> Option<usize> {
        match &self.nodes[dir].body {
            Body::Dir(e) => e.iter().find(|(n, _)| n == name).map(|(_, i)| *i),
            _ => None,
        }
    }
    fn add_child(&mut self, dir: usize, name: &[u8], idx: usize) {
        if let Body::Dir(e) = &mut self.nodes[dir].body {
            e.push((name.to_vec(), idx));
        }
    }
    fn alloc(&mut self, body: Body, mode: u32) -> usize {
        let now = self.now;
        self.nodes.push(Node { body, mode, mtime: now, nlink: 1 });
        self.nodes.len() - 1
    }
}

/// Normalise a path against `cwd` into absolute components, resolving "."/"..".
fn resolve(fs: &Fs, cwd: &[u8], path: &[u8], follow_last: bool) -> Result<usize, i32> {
    let (start, rest) = if path.first() == Some(&b'/') { (0usize, path) } else { (0usize, &b""[..]) };
    let mut cur = start;
    // Build the component list from cwd (if relative) then the path.
    let mut comps: Vec<&[u8]> = Vec::new();
    if path.first() != Some(&b'/') {
        for c in cwd.split(|&b| b == b'/') {
            if !c.is_empty() {
                comps.push(c);
            }
        }
    }
    let _ = rest;
    for c in path.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                comps.pop();
            }
            _ => comps.push(c),
        }
    }
    let n = comps.len();
    for (depth, c) in comps.iter().enumerate() {
        let next = fs.child(cur, c).ok_or(ENOENT)?;
        let is_last = depth + 1 == n;
        // Follow symlinks on intermediate components, and on the last if asked.
        if matches!(fs.nodes[next].body, Body::Link(_)) && (!is_last || follow_last) {
            if let Body::Link(t) = &fs.nodes[next].body {
                let base = {
                    // Links resolve relative to their parent directory.
                    let mut p = Vec::new();
                    for cc in &comps[..depth] {
                        p.push(b'/');
                        p.extend_from_slice(cc);
                    }
                    if p.is_empty() {
                        p.push(b'/');
                    }
                    p
                };
                cur = resolve(fs, &base, t, true)?;
            }
        } else {
            cur = next;
        }
    }
    Ok(cur)
}

fn split_parent(path: &[u8]) -> (&[u8], &[u8]) {
    let p = path.strip_suffix(b"/").unwrap_or(path);
    match p.iter().rposition(|&b| b == b'/') {
        Some(0) => (b"/", &p[1..]),
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (b".", p),
    }
}

pub fn lookup(cwd: &[u8], path: &[u8]) -> Result<usize, i32> {
    with(|fs| resolve(fs, cwd, path, true))
}

pub fn lookup_nofollow(cwd: &[u8], path: &[u8]) -> Result<usize, i32> {
    with(|fs| resolve(fs, cwd, path, false))
}

pub fn stat_idx(idx: usize) -> Stat {
    with(|fs| {
        let n = &fs.nodes[idx];
        let (mode_kind, size) = match &n.body {
            Body::Dir(_) => (S_IFDIR, 0),
            Body::File(d) => (S_IFREG, d.len() as u64),
            Body::Link(t) => (S_IFLNK, t.len() as u64),
        };
        Stat { mode: mode_kind | (n.mode & 0o7777), size, mtime: n.mtime, ino: idx as u64, nlink: n.nlink }
    })
}

pub fn is_dir(idx: usize) -> bool {
    with(|fs| matches!(fs.nodes[idx].body, Body::Dir(_)))
}

pub fn read_all(idx: usize) -> Option<Vec<u8>> {
    with(|fs| match &fs.nodes[idx].body {
        Body::File(d) => Some(d.clone()),
        _ => None,
    })
}

/// Read up to buf.len() bytes from a file at offset; returns bytes read.
pub fn read_at(idx: usize, off: usize, buf: &mut [u8]) -> usize {
    with(|fs| match &fs.nodes[idx].body {
        Body::File(d) if off < d.len() => {
            let n = buf.len().min(d.len() - off);
            buf[..n].copy_from_slice(&d[off..off + n]);
            n
        }
        _ => 0,
    })
}

pub fn file_len(idx: usize) -> usize {
    with(|fs| match &fs.nodes[idx].body {
        Body::File(d) => d.len(),
        _ => 0,
    })
}

/// Write at offset, growing the file; returns bytes written or <0 on error.
pub fn write_at(idx: usize, off: usize, data: &[u8]) -> i32 {
    with(|fs| {
        let now = fs.now;
        match &mut fs.nodes[idx].body {
            Body::File(d) => {
                if off + data.len() > d.len() {
                    d.resize(off + data.len(), 0);
                }
                d[off..off + data.len()].copy_from_slice(data);
                fs.nodes[idx].mtime = now;
                data.len() as i32
            }
            _ => EISDIR,
        }
    })
}

pub fn truncate(idx: usize) {
    with(|fs| {
        if let Body::File(d) = &mut fs.nodes[idx].body {
            d.clear();
        }
    });
}

/// Create a regular file (or return the existing one unless `excl`).
pub fn create(cwd: &[u8], path: &[u8], mode: u32, excl: bool) -> Result<usize, i32> {
    with(|fs| {
        if let Ok(idx) = resolve(fs, cwd, path, true) {
            if excl {
                return Err(EEXIST);
            }
            if matches!(fs.nodes[idx].body, Body::Dir(_)) {
                return Err(EISDIR);
            }
            return Ok(idx);
        }
        let (parent, name) = split_parent(path);
        if name.is_empty() {
            return Err(EINVAL);
        }
        let pidx = resolve(fs, cwd, parent, true)?;
        if !matches!(fs.nodes[pidx].body, Body::Dir(_)) {
            return Err(ENOTDIR);
        }
        let idx = fs.alloc(Body::File(Vec::new()), mode & 0o7777);
        fs.add_child(pidx, name, idx);
        Ok(idx)
    })
}

pub fn mkdir(path: &[u8], mode: u32) -> i32 {
    mkdir_at(b"/", path, mode)
}

pub fn mkdir_at(cwd: &[u8], path: &[u8], mode: u32) -> i32 {
    with(|fs| {
        if resolve(fs, cwd, path, true).is_ok() {
            return EEXIST;
        }
        let (parent, name) = split_parent(path);
        if name.is_empty() {
            return EINVAL;
        }
        let pidx = match resolve(fs, cwd, parent, true) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if !matches!(fs.nodes[pidx].body, Body::Dir(_)) {
            return ENOTDIR;
        }
        let idx = fs.alloc(Body::Dir(Vec::new()), mode & 0o7777);
        fs.add_child(pidx, name, idx);
        0
    })
}

pub fn symlink(cwd: &[u8], target: &[u8], path: &[u8]) -> i32 {
    with(|fs| {
        if resolve(fs, cwd, path, false).is_ok() {
            return EEXIST;
        }
        let (parent, name) = split_parent(path);
        let pidx = match resolve(fs, cwd, parent, true) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let idx = fs.alloc(Body::Link(target.to_vec()), 0o777);
        fs.add_child(pidx, name, idx);
        0
    })
}

pub fn readlink(idx: usize) -> Option<Vec<u8>> {
    with(|fs| match &fs.nodes[idx].body {
        Body::Link(t) => Some(t.clone()),
        _ => None,
    })
}

fn unlink_generic(cwd: &[u8], path: &[u8], want_dir: bool) -> i32 {
    with(|fs| {
        let (parent, name) = split_parent(path);
        if name == b"." || name == b".." || name.is_empty() {
            return EINVAL;
        }
        let pidx = match resolve(fs, cwd, parent, true) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let cidx = match fs.child(pidx, name) {
            Some(i) => i,
            None => return ENOENT,
        };
        let is_dir = matches!(fs.nodes[cidx].body, Body::Dir(_));
        if want_dir && !is_dir {
            return ENOTDIR;
        }
        if !want_dir && is_dir {
            return EISDIR;
        }
        if is_dir {
            if let Body::Dir(e) = &fs.nodes[cidx].body {
                if !e.is_empty() {
                    return ENOTEMPTY;
                }
            }
        }
        if let Body::Dir(e) = &mut fs.nodes[pidx].body {
            e.retain(|(n, _)| n != name);
        }
        0
    })
}

pub fn unlink(cwd: &[u8], path: &[u8]) -> i32 {
    unlink_generic(cwd, path, false)
}

pub fn rmdir(cwd: &[u8], path: &[u8]) -> i32 {
    unlink_generic(cwd, path, true)
}

pub fn rename(cwd: &[u8], from: &[u8], to: &[u8]) -> i32 {
    with(|fs| {
        let (fp, fname) = split_parent(from);
        let fpidx = match resolve(fs, cwd, fp, true) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let cidx = match fs.child(fpidx, fname) {
            Some(i) => i,
            None => return ENOENT,
        };
        let (tp, tname) = split_parent(to);
        let tpidx = match resolve(fs, cwd, tp, true) {
            Ok(i) => i,
            Err(e) => return e,
        };
        // Remove any existing destination, then relink.
        if let Body::Dir(e) = &mut fs.nodes[tpidx].body {
            e.retain(|(n, _)| n != tname);
        }
        if let Body::Dir(e) = &mut fs.nodes[fpidx].body {
            e.retain(|(n, _)| n != fname);
        }
        fs.add_child(tpidx, tname, cidx);
        0
    })
}

pub fn chmod(idx: usize, mode: u32) {
    with(|fs| fs.nodes[idx].mode = mode & 0o7777);
}

pub fn touch(idx: usize) {
    with(|fs| {
        let now = fs.now;
        fs.nodes[idx].mtime = now;
    });
}

/// Directory entry names (excluding "." and ".."), unsorted.
pub fn readdir(idx: usize) -> Option<Vec<Vec<u8>>> {
    with(|fs| match &fs.nodes[idx].body {
        Body::Dir(e) => Some(e.iter().map(|(n, _)| n.clone()).collect()),
        _ => None,
    })
}
