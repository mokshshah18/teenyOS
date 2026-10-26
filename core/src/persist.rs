// Persistence. teenyOS core keeps everything in RAM; this saves the home
// directory (/root) to a single archive file on the FAT volume it booted from,
// using the firmware's filesystem -- so there is no FAT driver here either.
// The archive is loaded at boot and written back on power off (or `persist
// save`), so your files come back.

use crate::rt::out;
use crate::{uefi, vfs};
use crate::util::*;
use alloc::vec::Vec;

const ARCHIVE: &[u8] = b"\\teenyos\\home.dat";
const MAGIC: &[u8; 8] = b"TEENYv1\n";

/// Serialize every file under /root into one archive buffer.
fn pack() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    pack_dir(b"/root", b"", &mut out);
    out
}

fn pack_dir(abs: &[u8], rel: &[u8], out: &mut Vec<u8>) {
    let mut names = match crate::sysb::list_dir(abs) {
        Ok(n) => n,
        Err(_) => return,
    };
    names.sort();
    for n in names {
        let child_abs = join(abs, &n);
        let mut child_rel = rel.to_vec();
        if !child_rel.is_empty() {
            child_rel.push(b'/');
        }
        child_rel.extend_from_slice(&n);
        match crate::sysb::stat(&child_abs) {
            Some(st) if st.is_dir() => {
                // Directory marker: type 'D', path, no data.
                out.push(b'D');
                put_bytes(out, &child_rel);
                pack_dir(&child_abs, &child_rel, out);
            }
            Some(st) if !st.is_lnk() => {
                if let Ok(data) = read_file(&child_abs) {
                    out.push(b'F');
                    put_bytes(out, &child_rel);
                    put_u32(out, data.len() as u32);
                    out.extend_from_slice(&data);
                }
            }
            _ => {}
        }
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn get_u32(d: &[u8], p: &mut usize) -> Option<u32> {
    let v = u32::from_le_bytes(d.get(*p..*p + 4)?.try_into().ok()?);
    *p += 4;
    Some(v)
}

fn get_bytes<'a>(d: &'a [u8], p: &mut usize) -> Option<&'a [u8]> {
    let n = get_u32(d, p)? as usize;
    let s = d.get(*p..*p + n)?;
    *p += n;
    Some(s)
}

/// Recreate the saved tree under /root. Returns the number of files restored.
fn unpack(d: &[u8]) -> usize {
    if !d.starts_with(MAGIC) {
        return 0;
    }
    let mut p = MAGIC.len();
    let mut files = 0;
    while p < d.len() {
        let kind = d[p];
        p += 1;
        let Some(rel) = get_bytes(d, &mut p) else { break };
        let abs = join(b"/root", rel);
        match kind {
            b'D' => {
                vfs::mkdir_at(b"/", &abs, 0o755);
            }
            b'F' => {
                let Some(n) = get_u32(d, &mut p) else { break };
                let Some(data) = d.get(p..p + n as usize) else { break };
                p += n as usize;
                let _ = write_file(&abs, data);
                files += 1;
            }
            _ => break,
        }
    }
    files
}

/// Called once at boot: restore the archive if present.
pub fn load_at_boot() {
    if let Some(d) = uefi::fs_read(ARCHIVE) {
        unpack(&d);
    }
}

/// Save /root now. Returns true on success.
pub fn save() -> bool {
    uefi::fs_write(ARCHIVE, &pack())
}

pub fn main(args: &[&[u8]]) -> i32 {
    match args.get(1).copied() {
        Some(b"save") | Some(b"on") => {
            out(b"Saving /root to the boot disk...\n");
            crate::rt::flush();
            if save() {
                out(b"Saved. Your files in /root will come back after a reboot\n");
                out(b"(they are also saved automatically on power off).\n");
                0
            } else {
                out(b"persist: could not write to the boot disk (is it read-only?)\n");
                1
            }
        }
        Some(b"load") => {
            load_at_boot();
            out(b"Reloaded saved files into /root.\n");
            0
        }
        _ => {
            let present = uefi::fs_read(ARCHIVE).is_some();
            out(b"Persistence saves /root to the USB/boot disk.\n");
            out(if present {
                b"A saved copy exists; it is loaded at every boot.\n"
            } else {
                b"Nothing saved yet. Run `persist save` to keep your files.\n"
            });
            0
        }
    }
}
