// Disks: finding them, partitioning, formatting FAT32, and locating files
// on disk for the BIOS boot loader.

use crate::sys;
use crate::util::*;
use alloc::vec;
use alloc::vec::Vec;

pub const SECTOR: u64 = 512;

pub struct Disk {
    pub name: Vec<u8>,
    pub sectors: u64,
    pub removable: bool,
    pub model: Vec<u8>,
}

pub struct Part {
    pub dev: Vec<u8>, // e.g. "sdb1"
    pub disk: Vec<u8>,
}

fn sysfs_u64(path: &[u8]) -> u64 {
    read_file(path).ok().and_then(|d| parse_u64(trim(&d))).unwrap_or(0)
}

pub fn disks() -> Vec<Disk> {
    let mut v = Vec::new();
    let mut names = list_dir(b"/sys/block").unwrap_or_default();
    names.sort();
    for n in names {
        if n.starts_with(b"loop") || n.starts_with(b"ram") {
            continue;
        }
        let base = join(b"/sys/block", &n);
        let sectors = sysfs_u64(&join(&base, b"size"));
        if sectors == 0 {
            continue;
        }
        let vendor = read_file(&join(&base, b"device/vendor")).unwrap_or_default();
        let model = read_file(&join(&base, b"device/model")).unwrap_or_default();
        let mut m = trim(&vendor).to_vec();
        if !m.is_empty() {
            m.push(b' ');
        }
        m.extend_from_slice(trim(&model));
        v.push(Disk { removable: sysfs_u64(&join(&base, b"removable")) == 1, name: n, sectors, model: m });
    }
    v
}

pub fn parts() -> Vec<Part> {
    let mut v = Vec::new();
    for d in disks() {
        let base = join(b"/sys/block", &d.name);
        let mut subs = list_dir(&base).unwrap_or_default();
        subs.sort();
        for s in subs {
            let pb = join(&base, &s);
            if sys::stat(&join(&pb, b"partition")).is_none() {
                continue;
            }
            v.push(Part { dev: s, disk: d.name.clone() });
        }
    }
    v
}

/// Partition device name: sdb + 2 = sdb2, nvme0n1 + 2 = nvme0n1p2.
pub fn part_name(disk: &[u8], n: u32) -> Vec<u8> {
    let mut v = disk.to_vec();
    if disk.last().map_or(false, |c| c.is_ascii_digit()) {
        v.push(b'p');
    }
    v.extend_from_slice(&num(n as u64));
    v
}

pub fn dev(name: &[u8]) -> Vec<u8> {
    join(b"/dev", name)
}

pub fn mount_fat(devname: &[u8], dir: &[u8], ro: bool) -> isize {
    sys::mkdir(dir, 0o755);
    // "flush" writes data out promptly, for sticks pulled without shutdown.
    sys::mount(&dev(devname), dir, b"vfat", if ro { 1 } else { 0 }, b"flush,shortname=mixed")
}

pub fn human_size(sectors: u64) -> Vec<u8> {
    human(sectors * SECTOR)
}

// ---------------------------------------------------------------- raw I/O

pub fn read_at(fd: i32, off: u64, buf: &mut [u8]) -> bool {
    sys::sc(sys::SYS_PREAD, fd as usize, buf.as_mut_ptr() as usize, buf.len(), off as usize, 0) == buf.len() as isize
}

pub fn write_at(fd: i32, off: u64, buf: &[u8]) -> bool {
    sys::sc(sys::SYS_PWRITE, fd as usize, buf.as_ptr() as usize, buf.len(), off as usize, 0) == buf.len() as isize
}

pub fn fsync(fd: i32) {
    sys::sc(sys::SYS_FSYNC, fd as usize, 0, 0, 0, 0);
}

const BLKRRPART: usize = 0x125f;
const BLKPG: usize = 0x1269;

/// Ask the kernel to re-read a disk's partition table (fails if any
/// partition on it is mounted).
pub fn reread(fd: i32) -> isize {
    sys::ioctl(fd, BLKRRPART, 0)
}

/// Tell the kernel about one new partition without re-reading the table,
/// which works even while other partitions on the disk are mounted.
pub fn add_part(fd: i32, num: u32, start: u64, sectors: u64) -> isize {
    // struct blkpg_partition { long long start, length; int pno;
    //                          char devname[64], volname[64]; }
    let mut p = [0u8; 8 + 8 + 4 + 64 + 64 + 4];
    p[0..8].copy_from_slice(&(start * SECTOR).to_le_bytes());
    p[8..16].copy_from_slice(&(sectors * SECTOR).to_le_bytes());
    p[16..20].copy_from_slice(&num.to_le_bytes());
    // struct blkpg_ioctl_arg { int op, flags, datalen; void *data; }
    let mut a = [0u8; 24];
    a[0..4].copy_from_slice(&1u32.to_le_bytes()); // BLKPG_ADD_PARTITION
    a[8..12].copy_from_slice(&(p.len() as u32).to_le_bytes());
    a[16..24].copy_from_slice(&(p.as_ptr() as u64).to_le_bytes());
    sys::ioctl(fd, BLKPG, a.as_ptr() as usize)
}

/// Wait for a device node to appear (the kernel creates it asynchronously).
pub fn wait_dev(name: &[u8], ms: u64) -> bool {
    let t = sys::now_ms();
    while sys::now_ms() - t < ms {
        if sys::stat(&dev(name)).is_some() {
            return true;
        }
        sys::sleep_ms(50);
    }
    false
}

// ---------------------------------------------------------------- MBR

pub struct MbrEntry {
    pub boot: bool,
    pub kind: u8,
    pub start: u64,
    pub sectors: u64,
}

pub fn mbr_entries(mbr: &[u8]) -> Vec<MbrEntry> {
    let mut v = Vec::new();
    if mbr.len() < 512 || mbr[510] != 0x55 || mbr[511] != 0xaa {
        return v;
    }
    for i in 0..4 {
        let e = &mbr[446 + i * 16..462 + i * 16];
        let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64;
        let sectors = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
        v.push(MbrEntry { boot: e[0] == 0x80, kind: e[4], start, sectors });
    }
    v
}

pub fn set_mbr_entry(mbr: &mut [u8], i: usize, e: &MbrEntry) {
    let o = 446 + i * 16;
    mbr[o] = if e.boot { 0x80 } else { 0 };
    mbr[o + 1..o + 4].copy_from_slice(b"\xfe\xff\xff");
    mbr[o + 4] = e.kind;
    mbr[o + 5..o + 8].copy_from_slice(b"\xfe\xff\xff");
    mbr[o + 8..o + 12].copy_from_slice(&(e.start as u32).to_le_bytes());
    mbr[o + 12..o + 16].copy_from_slice(&(e.sectors.min(0xffff_ffff) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
}

// ---------------------------------------------------------------- FAT32

/// Format `sectors` sectors starting at byte `off` of `fd` as FAT32.
/// `hidden` is the partition's starting LBA, recorded in the boot sector.
pub fn mkfs_fat32(fd: i32, off: u64, sectors: u64, hidden: u64, label: &[u8; 11]) -> bool {
    if sectors < 66_600 {
        return false; // FAT32 needs at least 65525 clusters
    }
    let mut spc: u64 = match sectors * SECTOR {
        s if s <= 8 << 30 => 8,
        s if s <= 16 << 30 => 16,
        s if s <= 32 << 30 => 32,
        _ => 64,
    };
    // Small partitions need smaller clusters to reach FAT32's minimum count;
    // otherwise other systems may take the volume for FAT16.
    while spc > 1 && sectors / spc < 66_000 {
        spc /= 2;
    }
    let reserved: u64 = 32;
    // Microsoft's FAT32 sizing formula (fatgen103).
    let t1 = sectors - reserved;
    let t2 = (256 * spc + 2) / 2;
    let fatsz = (t1 + t2 - 1) / t2;
    let total = sectors.min(0xffff_ffff);

    let mut bs = vec![0u8; 512];
    bs[0..3].copy_from_slice(b"\xeb\x58\x90");
    bs[3..11].copy_from_slice(b"TEENYOS ");
    bs[11..13].copy_from_slice(&512u16.to_le_bytes());
    bs[13] = spc as u8;
    bs[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
    bs[16] = 2; // FATs
    bs[21] = 0xf8; // media
    bs[24..26].copy_from_slice(&63u16.to_le_bytes());
    bs[26..28].copy_from_slice(&255u16.to_le_bytes());
    bs[28..32].copy_from_slice(&(hidden as u32).to_le_bytes());
    bs[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    bs[36..40].copy_from_slice(&(fatsz as u32).to_le_bytes());
    bs[44..48].copy_from_slice(&2u32.to_le_bytes()); // root cluster
    bs[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
    bs[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
    bs[64] = 0x80;
    bs[66] = 0x29;
    bs[67..71].copy_from_slice(&(sys::now() as u32 ^ 0x7e3d_1f05).to_le_bytes());
    bs[71..82].copy_from_slice(label);
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xaa;

    let mut fsinfo = vec![0u8; 512];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    fsinfo[508..512].copy_from_slice(&0xaa55_0000u32.to_le_bytes());

    // Zero the reserved area, both FATs and the root directory cluster.
    let zero = vec![0u8; 64 * 1024];
    let mut left = (reserved + 2 * fatsz + spc) * SECTOR;
    let mut pos = off;
    while left > 0 {
        let n = left.min(zero.len() as u64) as usize;
        if !write_at(fd, pos, &zero[..n]) {
            return false;
        }
        pos += n as u64;
        left -= n as u64;
    }
    let mut ok = write_at(fd, off, &bs) && write_at(fd, off + SECTOR, &fsinfo);
    ok &= write_at(fd, off + 6 * SECTOR, &bs) && write_at(fd, off + 7 * SECTOR, &fsinfo);
    let mut fat0 = [0u8; 12];
    fat0[0..4].copy_from_slice(&0x0fff_fff8u32.to_le_bytes());
    fat0[4..8].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());
    fat0[8..12].copy_from_slice(&0x0fff_ffffu32.to_le_bytes()); // root directory
    for i in 0..2 {
        ok &= write_at(fd, off + (reserved + i * fatsz) * SECTOR, &fat0);
    }
    // Volume label entry, so the label also shows in other systems.
    let mut root = [0u8; 32];
    root[0..11].copy_from_slice(label);
    root[11] = 0x08;
    ok &= write_at(fd, off + (reserved + 2 * fatsz) * SECTOR, &root);
    fsync(fd);
    ok
}

/// Disk sectors (relative to the start of the filesystem) occupied by a
/// file, or None if the file is not stored contiguously.
pub fn file_extent(path: &[u8]) -> Option<(u64, u64)> {
    const FIBMAP: usize = 1;
    const FIGETBSZ: usize = 2;
    let fd = sys::open(path, sys::O_RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let size = sys::stat(path)?.size();
    let mut bsz: i32 = 0;
    sys::ioctl(fd, FIGETBSZ, &mut bsz as *mut i32 as usize);
    let bsz = bsz.max(512) as u64;
    let blocks = size.div_ceil(bsz);
    let mut first = 0u64;
    let mut ok = true;
    for i in 0..blocks {
        let mut b: i32 = i as i32;
        if sys::ioctl(fd, FIBMAP, &mut b as *mut i32 as usize) < 0 {
            ok = false;
            break;
        }
        if i == 0 {
            first = b as u64;
        } else if b as u64 != first + i {
            ok = false;
            break;
        }
    }
    sys::close(fd);
    if !ok {
        return None;
    }
    Some((first * bsz / SECTOR, size.div_ceil(SECTOR)))
}

pub fn copy(src: &[u8], dst: &[u8]) -> bool {
    match read_file(src) {
        Ok(d) => write_file(dst, &d).is_ok(),
        Err(_) => false,
    }
}

/// Copy a directory tree (files and folders only; FAT has no symlinks).
pub fn copy_tree(src: &[u8], dst: &[u8]) {
    sys::mkdir(dst, 0o755);
    for n in list_dir(src).unwrap_or_default() {
        let (s, d) = (join(src, &n), join(dst, &n));
        match sys::lstat(&s) {
            Some(st) if st.is_dir() => copy_tree(&s, &d),
            Some(st) if !st.is_lnk() => {
                copy(&s, &d);
            }
            _ => {}
        }
    }
}
