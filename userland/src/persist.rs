// Persistence: /root lives in a folder called teenyos/data on a FAT disk
// (the USB stick, a data partition on it, or a PC's EFI partition).
//
// At boot init calls boot_scan(), which finds the teenyOS boot partition and
// any saved data. `persist enable` / `persist disable` switch it at run time.

use crate::disk::{self, Part};
use crate::rt::{flush, out};
use crate::sys;
use crate::util::*;
use alloc::vec::Vec;

const PROBE: &[u8] = b"/mnt/probe";
const DATA_MNT: &[u8] = b"/mnt/data";
const MS_BIND: usize = 4096;
const MNT_DETACH: usize = 2;

fn is_dir(p: &[u8]) -> bool {
    sys::stat(p).map_or(false, |s| s.is_dir())
}

struct Probe {
    teeny: bool,  // has a teenyos/ folder and a teenyOS kernel: we booted from here
    data: bool,   // has teenyos/data
    off: bool,    // has teenyos/data.off (persistence turned off earlier)
}

fn probe(p: &Part) -> Option<Probe> {
    if disk::mount_fat(&p.dev, PROBE, true) < 0 {
        return None;
    }
    let j = |s: &[u8]| join(PROBE, s);
    let kernel = sys::stat(&j(b"EFI/BOOT/BOOTX64.EFI")).is_some() || sys::stat(&j(b"EFI/teenyOS/teenyOS.efi")).is_some();
    let r = Probe { teeny: is_dir(&j(b"teenyos")) && kernel, data: is_dir(&j(b"teenyos/data")), off: is_dir(&j(b"teenyos/data.off")) };
    sys::umount(PROBE);
    Some(r)
}

/// Mount `dev` and put its teenyos/data folder over /root.
fn activate(dev: &[u8], first_time: bool) -> bool {
    if disk::mount_fat(dev, DATA_MNT, false) < 0 {
        return false;
    }
    let data = join(DATA_MNT, b"teenyos/data");
    sys::mkdir(&join(DATA_MNT, b"teenyos"), 0o755);
    sys::mkdir(&data, 0o755);
    if first_time {
        // Carry over whatever is in the RAM home directory now.
        disk::copy_tree(b"/root", &data);
    }
    if sys::mount(&data, b"/root", b"", MS_BIND, b"") < 0 {
        sys::umount(DATA_MNT);
        return false;
    }
    sys::sync();
    let _ = write_file(b"/run/persist", dev);
    true
}

pub fn boot_device() -> Vec<u8> {
    read_file(b"/run/bootdev").map(|d| trim(&d).to_vec()).unwrap_or_default()
}

/// Called by init before any login starts. USB sticks show up a moment
/// after boot, so keep looking for a few seconds until we have found the
/// boot partition (or saved data).
pub fn boot_scan(cmdline: &[u8]) {
    if !is_dir(b"/sys/block") {
        return; // the mini build has no disk support
    }
    let mut wait_ms = 5000;
    if let Some(i) = find(cmdline, b"teeny.wait=") {
        let v = &cmdline[i + 11..];
        let end = v.iter().position(|c| c.is_ascii_whitespace()).unwrap_or(v.len());
        wait_ms = parse_u64(&v[..end]).unwrap_or(5000) * 1000;
    }
    let t0 = sys::now_ms();
    let mut seen: Vec<Vec<u8>> = Vec::new();
    let mut boot: Option<Vec<u8>> = None;
    let mut told = false;
    loop {
        for p in disk::parts() {
            if seen.contains(&p.dev) {
                continue;
            }
            seen.push(p.dev.clone());
            let Some(pr) = probe(&p) else { continue };
            if pr.teeny && boot.is_none() {
                boot = Some(p.dev.clone());
                let _ = write_file(b"/run/bootdev", &p.dev);
            }
            if pr.data {
                if activate(&p.dev, false) {
                    out(b"teenyOS: restored your files from ");
                    out(&p.dev);
                    out(b"\n");
                    flush();
                }
                return;
            }
        }
        if boot.is_some() || sys::now_ms() - t0 >= wait_ms {
            return;
        }
        if !told && sys::now_ms() - t0 > 800 {
            out(b"teenyOS: looking for the boot disk...\n");
            flush();
            told = true;
        }
        sys::sleep_ms(150);
    }
}

fn ask(q: &[u8]) -> bool {
    out(q);
    out(b" [y/N] ");
    flush();
    let mut line = Vec::new();
    let mut c = [0u8; 1];
    while sys::read(0, &mut c) == 1 && c[0] != b'\n' {
        line.push(c[0]);
    }
    matches!(trim(&line), b"y" | b"Y" | b"yes" | b"YES" | b"Yes")
}

/// Unused space at the end of an MBR disk: (first free slot, start, sectors).
fn free_space(disk_name: &[u8]) -> Option<(usize, u64, u64)> {
    let total = disk::disks().into_iter().find(|d| d.name == disk_name)?.sectors;
    let fd = sys::open(&disk::dev(disk_name), sys::O_RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let mut mbr = [0u8; 512];
    let ok = disk::read_at(fd, 0, &mut mbr);
    sys::close(fd);
    if !ok {
        return None;
    }
    let ents = disk::mbr_entries(&mbr);
    if ents.is_empty() || ents.iter().any(|e| e.kind == 0xee) {
        return None; // not MBR (GPT disks are left alone)
    }
    let slot = ents.iter().position(|e| e.kind == 0)?;
    let end = ents.iter().filter(|e| e.kind != 0).map(|e| e.start + e.sectors).max().unwrap_or(2048);
    let start = end.div_ceil(2048) * 2048;
    let sectors = total.saturating_sub(start).min(0xffff_ffff) / 2048 * 2048;
    if sectors < 128 * 1024 {
        return None; // under 64 MiB is not worth a partition
    }
    Some((slot, start, sectors))
}

fn create_data_partition(disk_name: &[u8], slot: usize, start: u64, sectors: u64) -> Option<Vec<u8>> {
    let fd = sys::open(&disk::dev(disk_name), sys::O_RDWR, 0);
    if fd < 0 {
        return None;
    }
    let mut mbr = [0u8; 512];
    if !disk::read_at(fd, 0, &mut mbr) {
        sys::close(fd);
        return None;
    }
    disk::set_mbr_entry(&mut mbr, slot, &disk::MbrEntry { boot: false, kind: 0x0c, start, sectors });
    let num = slot as u32 + 1;
    let name = disk::part_name(disk_name, num);
    out(b"Formatting ");
    out(&name);
    out(b" as FAT32 (label TEENYDATA)...\n");
    flush();
    let ok = disk::write_at(fd, 0, &mbr) && disk::mkfs_fat32(fd, start * disk::SECTOR, sectors, start, b"TEENYDATA  ");
    disk::fsync(fd);
    let r = disk::add_part(fd, num, start, sectors);
    sys::close(fd);
    if !ok || (r < 0 && sys::stat(&disk::dev(&name)).is_none()) || !disk::wait_dev(&name, 3000) {
        return None;
    }
    Some(name)
}

fn enable(args: &[&[u8]]) -> i32 {
    let cur = read_file(b"/run/persist").unwrap_or_default();
    if !cur.is_empty() {
        out(b"Persistence is already on: /root is saved on ");
        out(&cur);
        out(b".\n");
        return 0;
    }
    // An explicit partition: "persist enable sdb1" or "/dev/sdb1".
    if let Some(&d) = args.get(2) {
        let d = d.strip_prefix(b"/dev/").unwrap_or(d);
        if !activate(d, true) {
            msg(b"persist", b"could not use that partition (it must be FAT formatted)");
            return 1;
        }
        return done(d);
    }
    // Turned off earlier? Bring that data back.
    for p in disk::parts() {
        if let Some(pr) = probe(&p) {
            if pr.off && disk::mount_fat(&p.dev, PROBE, false) >= 0 {
                sys::rename(&join(PROBE, b"teenyos/data.off"), &join(PROBE, b"teenyos/data"));
                sys::umount(PROBE);
                if activate(&p.dev, false) {
                    out(b"Found the files you saved before.\n");
                    return done(&p.dev);
                }
            }
        }
    }
    let boot = boot_device();
    if boot.is_empty() {
        msg(b"persist", b"can't tell which disk teenyOS booted from.");
        out(b"Name a FAT partition instead, for example: persist enable sdb1\n");
        return 1;
    }
    let bp = disk::parts().into_iter().find(|p| p.dev == boot);
    let disk_name = bp.as_ref().map(|p| p.disk.clone()).unwrap_or_default();
    // Prefer a data partition in the stick's free space: much more room.
    if let Some((slot, start, sectors)) = free_space(&disk_name) {
        out(b"The disk teenyOS booted from (");
        out(&disk_name);
        out(b") has ");
        out(&disk::human_size(sectors));
        out(b" of unused space.\n");
        if ask(b"Create a data partition there for your files?") {
            match create_data_partition(&disk_name, slot, start, sectors) {
                Some(name) if activate(&name, true) => return done(&name),
                _ => {
                    msg(b"persist", b"could not create the data partition");
                    return 1;
                }
            }
        }
    }
    out(b"Saving to a teenyos/data folder on ");
    out(&boot);
    out(b" instead.\n");
    if activate(&boot, true) {
        return done(&boot);
    }
    msg(b"persist", b"could not write to the boot partition");
    1
}

fn done(dev: &[u8]) -> i32 {
    out(b"Persistence is on: /root is now saved on ");
    out(dev);
    out(b" and comes back every time teenyOS starts.\n");
    out(b"(Open a new terminal, or cd ~, to see it.)\n");
    0
}

fn disable() -> i32 {
    let cur = read_file(b"/run/persist").unwrap_or_default();
    if cur.is_empty() {
        out(b"Persistence is already off.\n");
        return 0;
    }
    sys::sync();
    sys::sc(sys::SYS_UMOUNT2, sys::CStr::new(b"/root").p(), MNT_DETACH, 0, 0, 0);
    let r = sys::rename(&join(DATA_MNT, b"teenyos/data"), &join(DATA_MNT, b"teenyos/data.off"));
    sys::sync();
    sys::sc(sys::SYS_UMOUNT2, sys::CStr::new(DATA_MNT).p(), MNT_DETACH, 0, 0, 0);
    sys::unlink(b"/run/persist");
    if r < 0 {
        perr(b"persist", b"rename", r);
    }
    out(b"Persistence is off: from now on /root is in RAM only.\n");
    out(b"Your saved files stay in teenyos/data.off on ");
    out(&cur);
    out(b"; `persist enable` brings them back.\n");
    0
}

pub fn main(args: &[&[u8]]) -> i32 {
    match args.get(1).copied() {
        None | Some(b"status") => {
            let cur = read_file(b"/run/persist").unwrap_or_default();
            if cur.is_empty() {
                out(b"Persistence: off (everything is in RAM)\n");
            } else {
                out(b"Persistence: on, /root is saved on ");
                out(&cur);
                out(b"\n");
            }
            let boot = boot_device();
            if !boot.is_empty() {
                out(b"Booted from: ");
                out(&boot);
                out(b"\n");
            }
            0
        }
        Some(b"enable") | Some(b"on") => enable(args),
        Some(b"disable") | Some(b"off") => disable(),
        _ => {
            msg(b"persist", b"usage: persist [status | enable [PARTITION] | disable]");
            1
        }
    }
}
