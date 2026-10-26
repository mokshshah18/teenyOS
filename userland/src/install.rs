// teeny-install: put teenyOS on a disk.
//   teeny-install              list disks and explain the options
//   teeny-install DISK         erase DISK and make it a bootable teenyOS disk
//   teeny-install --alongside  add teenyOS to this PC's EFI partition and boot
//                              menu, leaving the installed OS alone

use crate::disk::{self, SECTOR};
use crate::persist::boot_device;
use crate::rt::{flush, out};
use crate::sys;
use crate::util::*;
use alloc::vec;
use alloc::vec::Vec;

const SRC: &[u8] = b"/mnt/src";
const DST: &[u8] = b"/mnt/target";
const EFI_GLOBAL: &[u8] = b"8be4df61-93ca-11d2-aa0d-00e098032b8c";
const ESP_GUID: [u8; 16] = [0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b];

fn confirm(q: &[u8]) -> bool {
    out(q);
    out(b"\nType yes to continue: ");
    flush();
    let mut line = Vec::new();
    let mut c = [0u8; 1];
    while sys::read(0, &mut c) == 1 && c[0] != b'\n' {
        line.push(c[0]);
    }
    trim(&line) == b"yes"
}

fn fail(m: &[u8]) -> i32 {
    msg(b"teeny-install", m);
    1
}

fn mounted(devname: &[u8]) -> bool {
    let m = read_file(b"/proc/mounts").unwrap_or_default();
    let d = disk::dev(devname);
    lines(&m).iter().any(|l| l.starts_with(&d))
}

/// The running system's kernel and BIOS loader, read from the boot disk.
struct Source {
    kernel: Vec<u8>,
    loader: Vec<u8>, // sector 0 code (440 bytes) + sectors 1..15
}

fn source() -> Result<Source, &'static [u8]> {
    let boot = boot_device();
    if boot.is_empty() {
        return Err(b"can't find the disk teenyOS booted from, so there is nothing to copy");
    }
    let part = disk::parts().into_iter().find(|p| p.dev == boot).ok_or(&b"boot partition vanished"[..])?;
    if disk::mount_fat(&boot, SRC, true) < 0 {
        return Err(b"can't read the boot partition");
    }
    let kernel = read_file(&join(SRC, b"EFI/BOOT/BOOTX64.EFI")).or_else(|_| read_file(&join(SRC, b"EFI/teenyOS/teenyOS.efi")));
    sys::umount(SRC);
    let kernel = kernel.map_err(|_| &b"no kernel on the boot partition"[..])?;
    let fd = sys::open(&disk::dev(&part.disk), sys::O_RDONLY, 0);
    let mut loader = vec![0u8; 16 * 512];
    let ok = fd >= 0 && disk::read_at(fd, 0, &mut loader);
    sys::close(fd);
    if !ok || &loader[512..520] != b"TEENYLDR" {
        // Booted from an alongside install: there is no BIOS loader to copy.
        loader.clear();
    }
    Ok(Source { kernel, loader })
}

fn usage() -> i32 {
    let boot = boot_device();
    out(b"Disks in this PC:\n");
    for d in disk::disks() {
        out(b"  ");
        out_padr(&d.name, 10);
        out_pad(&disk::human_size(d.sectors), 7);
        out(b"  ");
        out(&d.model);
        if d.removable {
            out(b"  (removable)");
        }
        if !boot.is_empty() && boot.starts_with(&d.name) {
            out(b"  <- teenyOS is running from this one");
        }
        out(b"\n");
    }
    out(b"\nUsage:\n");
    out(b"  teeny-install DISK         erase DISK and make it boot teenyOS, with your\n");
    out(b"                             files saved on it (e.g. teeny-install sdb)\n");
    out(b"  teeny-install --alongside  add teenyOS to this PC's UEFI boot menu and keep\n");
    out(b"                             the system that is installed now\n");
    0
}

fn whole_disk(name: &[u8]) -> i32 {
    let name = name.strip_prefix(b"/dev/").unwrap_or(name);
    let Some(d) = disk::disks().into_iter().find(|d| d.name == name) else { return fail(b"no such disk (run teeny-install to list them)") };
    let boot = boot_device();
    if !boot.is_empty() && boot.starts_with(name) && boot.len() > name.len() {
        return fail(b"that is the disk teenyOS is running from");
    }
    if disk::parts().iter().any(|p| p.disk == name && mounted(&p.dev)) {
        return fail(b"a partition on that disk is in use; turn persistence off first");
    }
    if d.sectors < 80 * 1024 {
        return fail(b"that disk is too small (teenyOS needs 40 MB)");
    }
    let src = match source() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if src.loader.is_empty() {
        return fail(b"this copy of teenyOS has no BIOS loader to install; boot from the USB stick");
    }
    let mut q = b"This ERASES EVERYTHING on ".to_vec();
    q.extend_from_slice(name);
    q.extend_from_slice(b" (");
    q.extend_from_slice(&disk::human_size(d.sectors));
    q.extend_from_slice(b" ");
    q.extend_from_slice(&d.model);
    q.extend_from_slice(b") and puts teenyOS on it.");
    if !confirm(&q) {
        out(b"Nothing was changed.\n");
        return 1;
    }

    let fd = sys::open(&disk::dev(name), sys::O_RDWR, 0);
    if fd < 0 {
        return fail(b"can't open the disk");
    }
    out(b"Writing the partition table and boot loader...\n");
    flush();
    // Wipe the first MiB and any backup GPT at the end of the disk.
    let zero = vec![0u8; 1024 * 1024];
    disk::write_at(fd, 0, &zero);
    disk::write_at(fd, (d.sectors - 64) * SECTOR, &zero[..64 * 512]);
    let mut mbr = vec![0u8; 512];
    mbr[..440].copy_from_slice(&src.loader[..440]);
    mbr[440..444].copy_from_slice(&(sys::now() as u32 ^ 0x7e3d_1f05).to_le_bytes());
    let start = 2048u64;
    let sectors = (d.sectors - start).min(0xffff_ffff);
    disk::set_mbr_entry(&mut mbr, 0, &disk::MbrEntry { boot: true, kind: 0x0c, start, sectors });
    let mut ok = disk::write_at(fd, 0, &mbr) && disk::write_at(fd, 512, &src.loader[512..]);
    out(b"Formatting...\n");
    flush();
    ok = ok && disk::mkfs_fat32(fd, start * SECTOR, sectors, start, b"TEENYOS    ");
    disk::fsync(fd);
    disk::reread(fd);
    let part = disk::part_name(name, 1);
    if !ok || !disk::wait_dev(&part, 5000) {
        sys::close(fd);
        return fail(b"writing the disk failed");
    }
    out(b"Copying teenyOS...\n");
    flush();
    if disk::mount_fat(&part, DST, false) < 0 {
        sys::close(fd);
        return fail(b"can't mount the new partition");
    }
    for dir in [&b"EFI"[..], b"EFI/BOOT", b"teenyos", b"teenyos/data"] {
        sys::mkdir(&join(DST, dir), 0o755);
    }
    let kpath = join(DST, b"EFI/BOOT/BOOTX64.EFI");
    ok = write_file(&kpath, &src.kernel).is_ok();
    let _ = write_file(&join(DST, b"teenyos/README.TXT"), b"teenyOS keeps your files (/root) in teenyos/data.\n");
    disk::copy_tree(b"/root", &join(DST, b"teenyos/data"));
    sys::sync();
    // Point the BIOS loader at the kernel file.
    let ext = disk::file_extent(&kpath);
    sys::umount(DST);
    let Some((rel, ksec)) = ext.filter(|_| ok) else {
        sys::close(fd);
        return fail(b"the kernel file did not land in one piece; try again");
    };
    let mut params = [0u8; 12];
    params[..8].copy_from_slice(&(start + rel).to_le_bytes());
    params[8..].copy_from_slice(&(ksec as u32).to_le_bytes());
    ok = disk::write_at(fd, 512 + 8, &params);
    disk::fsync(fd);
    sys::close(fd);
    if !ok {
        return fail(b"could not update the boot loader");
    }
    out(b"\nDone. teenyOS is installed on ");
    out(name);
    out(b" and will keep your files there.\n");
    out(b"Remove the USB stick and restart; pick this disk in the boot menu if needed.\n");
    0
}

// ---------------------------------------------------------------- alongside

struct Esp {
    dev: Vec<u8>,
    disk: Vec<u8>,
    num: u32,
    start: u64,
    sectors: u64,
    guid: [u8; 16],
}

/// Find EFI system partitions by reading each disk's GPT.
fn find_esps() -> Vec<Esp> {
    let mut v = Vec::new();
    for d in disk::disks() {
        let fd = sys::open(&disk::dev(&d.name), sys::O_RDONLY, 0);
        if fd < 0 {
            continue;
        }
        let mut h = [0u8; 512];
        if disk::read_at(fd, 512, &mut h) && &h[..8] == b"EFI PART" {
            let lba = u64::from_le_bytes(h[72..80].try_into().unwrap());
            let count = u32::from_le_bytes(h[80..84].try_into().unwrap()).min(256) as usize;
            let esz = u32::from_le_bytes(h[84..88].try_into().unwrap()).max(128) as usize;
            let mut t = vec![0u8; count * esz];
            if disk::read_at(fd, lba * SECTOR, &mut t) {
                for i in 0..count {
                    let e = &t[i * esz..i * esz + 128];
                    if e[..16] == ESP_GUID {
                        let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
                        let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
                        let num = i as u32 + 1;
                        v.push(Esp {
                            dev: disk::part_name(&d.name, num),
                            disk: d.name.clone(),
                            num,
                            start: first,
                            sectors: last - first + 1,
                            guid: e[16..32].try_into().unwrap(),
                        });
                    }
                }
            }
        }
        sys::close(fd);
    }
    v
}

fn utf16(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    for &c in s {
        v.push(c);
        v.push(0);
    }
    v.extend_from_slice(&[0, 0]);
    v
}

/// An EFI_LOAD_OPTION that boots \EFI\teenyOS\teenyOS.efi on the ESP.
fn load_option(esp: &Esp) -> Vec<u8> {
    let mut dp = Vec::new();
    // Hard drive media device path (GPT partition).
    dp.extend_from_slice(&[4, 1, 42, 0]);
    dp.extend_from_slice(&esp.num.to_le_bytes());
    dp.extend_from_slice(&esp.start.to_le_bytes());
    dp.extend_from_slice(&esp.sectors.to_le_bytes());
    dp.extend_from_slice(&esp.guid);
    dp.extend_from_slice(&[2, 2]);
    // File path.
    let path = utf16(b"\\EFI\\teenyOS\\teenyOS.efi");
    dp.extend_from_slice(&[4, 4]);
    dp.extend_from_slice(&((4 + path.len()) as u16).to_le_bytes());
    dp.extend_from_slice(&path);
    dp.extend_from_slice(&[0x7f, 0xff, 4, 0]);
    let mut opt = Vec::new();
    opt.extend_from_slice(&1u32.to_le_bytes()); // LOAD_OPTION_ACTIVE
    opt.extend_from_slice(&(dp.len() as u16).to_le_bytes());
    opt.extend_from_slice(&utf16(b"teenyOS"));
    opt.extend_from_slice(&dp);
    opt
}

fn hex4(n: u16) -> [u8; 4] {
    let h = b"0123456789ABCDEF";
    [h[(n >> 12) as usize & 15], h[(n >> 8) as usize & 15], h[(n >> 4) as usize & 15], h[n as usize & 15]]
}

fn efivar(name: &[u8]) -> Vec<u8> {
    [b"/sys/firmware/efi/efivars/" as &[u8], name, b"-", EFI_GLOBAL].concat()
}

/// Write an EFI variable (non-volatile, boot and runtime access).
fn set_efivar(name: &[u8], data: &[u8]) -> bool {
    let path = efivar(name);
    let mut buf = 7u32.to_le_bytes().to_vec();
    buf.extend_from_slice(data);
    let fd = sys::open(&path, sys::O_WRONLY | sys::O_CREAT, 0o644);
    if fd < 0 {
        return false;
    }
    // efivarfs wants the whole variable in a single write.
    let ok = sys::write(fd, &buf) == buf.len() as isize;
    sys::close(fd);
    ok
}

fn alongside() -> i32 {
    if !sys::stat(b"/sys/firmware/efi").is_some() {
        return fail(b"teenyOS was not started in UEFI mode, so it can't add itself to the boot menu. Use teeny-install DISK, or boot the stick in UEFI mode.");
    }
    let boot = boot_device();
    let esps: Vec<Esp> = find_esps().into_iter().filter(|e| !boot.starts_with(&e.disk)).collect();
    let Some(esp) = esps.into_iter().next() else { return fail(b"no EFI system partition found on this PC's disks") };
    let src = match source() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let mut q = b"Add teenyOS to the EFI system partition ".to_vec();
    q.extend_from_slice(&esp.dev);
    q.extend_from_slice(b" (");
    q.extend_from_slice(&disk::human_size(esp.sectors));
    q.extend_from_slice(b") and to the boot menu.\nIt adds one folder (EFI/teenyOS, about 2 MB) plus teenyos/ for your files;\nnothing else on the disk changes.");
    if !confirm(&q) {
        out(b"Nothing was changed.\n");
        return 1;
    }
    let was_mounted = mounted(&esp.dev);
    if was_mounted {
        return fail(b"the EFI partition is in use (turn persistence off first)");
    }
    if disk::mount_fat(&esp.dev, DST, false) < 0 {
        return fail(b"can't mount the EFI partition");
    }
    for dir in [&b"EFI"[..], b"EFI/teenyOS", b"teenyos", b"teenyos/data"] {
        sys::mkdir(&join(DST, dir), 0o755);
    }
    let ok = write_file(&join(DST, b"EFI/teenyOS/teenyOS.efi"), &src.kernel).is_ok();
    disk::copy_tree(b"/root", &join(DST, b"teenyos/data"));
    sys::sync();
    sys::umount(DST);
    if !ok {
        return fail(b"copying the kernel failed (is the EFI partition full?)");
    }

    sys::mount(b"efivarfs", b"/sys/firmware/efi/efivars", b"efivarfs", 0, b"");
    let names = list_dir(b"/sys/firmware/efi/efivars").unwrap_or_default();
    let mut n: u16 = 0;
    while names.iter().any(|v| v.starts_with(&[b"Boot" as &[u8], &hex4(n)].concat())) {
        n += 1;
    }
    let var = [b"Boot" as &[u8], &hex4(n)].concat();
    if !set_efivar(&var, &load_option(&esp)) {
        return fail(b"the firmware refused the new boot entry; teenyOS is copied, pick EFI/teenyOS/teenyOS.efi from your firmware's boot menu");
    }
    let order = read_file(&efivar(b"BootOrder")).unwrap_or_default();
    let mut entries: Vec<u16> = order.get(4..).unwrap_or(&[]).chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    entries.retain(|&e| e != n);
    out(b"\nMake teenyOS the default, so the PC starts it unless you pick otherwise?");
    let default = {
        out(b" [y/N] ");
        flush();
        let mut line = Vec::new();
        let mut c = [0u8; 1];
        while sys::read(0, &mut c) == 1 && c[0] != b'\n' {
            line.push(c[0]);
        }
        matches!(trim(&line), b"y" | b"Y" | b"yes")
    };
    if default {
        entries.insert(0, n);
    } else {
        entries.push(n);
    }
    let bytes: Vec<u8> = entries.iter().flat_map(|e| e.to_le_bytes()).collect();
    set_efivar(b"BootOrder", &bytes);
    out(b"\nDone. \"teenyOS\" is in this PC's boot menu");
    out(if default { b" and starts by default.\n" } else { b"; your current system still starts by default.\n" });
    out(b"Your files are kept in teenyos/data on the EFI partition.\n");
    0
}

pub fn main(args: &[&[u8]]) -> i32 {
    match args.get(1).copied() {
        None | Some(b"-h") | Some(b"--help") => usage(),
        Some(b"--alongside") => alongside(),
        Some(d) => whole_disk(d),
    }
}
