#!/usr/bin/env python3
"""Build the teenyOS USB stick image.

Layout (MBR partitioning, so both BIOS and UEFI PCs boot it):
  sector 0       boot loader stage 1 + partition table
  sectors 1-15   boot loader stage 2
  1 MiB onward   partition 1: FAT16, 8 MiB, label TEENYOS
                   EFI/BOOT/BOOTX64.EFI   the kernel (UEFI boots it directly)
                   teenyos/README.TXT

The kernel file is written to contiguous clusters and its disk position is
patched into stage 2, which is how the BIOS loader finds it.

usage: mkimage.py --loader loader.bin --kernel teenyOS.efi -o image
"""
import argparse
import struct

SECTOR = 512
PART_START = 2048          # 1 MiB
PART_SECTORS = 16384       # 8 MiB
SPC = 2                    # sectors per cluster (1 KiB clusters)
RESERVED = 4
ROOT_ENTRIES = 512
FATS = 2
FAT_SECTORS = 32
ROOT_SECTORS = ROOT_ENTRIES * 32 // SECTOR
DATA_START = RESERVED + FATS * FAT_SECTORS + ROOT_SECTORS
CLUSTERS = (PART_SECTORS - DATA_START) // SPC
# FAT date/time for 2026-01-01 00:00, fixed so builds are reproducible.
FAT_DATE = ((2026 - 1980) << 9) | (1 << 5) | 1
FAT_TIME = 0

README = b"""teenyOS
=======

This stick boots teenyOS on PCs with UEFI or legacy BIOS firmware.
Pick it from your PC's boot menu (often F12, F11, F8 or Esc at power on).

Your files are kept only in RAM until you turn on persistence: open the
Storage app on the desktop, or run `persist enable` in a terminal. That
creates a data partition in this stick's free space, or a teenyos/data
folder on a FAT disk, and saves /root there.

To put teenyOS on a PC's disk, use the Storage app or `teeny-install`.

Note for BIOS booting: the loader finds the kernel by its position on the
disk. If you replace EFI/BOOT/BOOTX64.EFI by hand, run teeny-install again
(UEFI does not care).
"""


def dirent(name, attr, cluster, size, lower=False):
    assert len(name) == 11
    ntres = 0x08 if lower else 0
    return struct.pack("<11sBBBHHHHHHHI", name, attr, ntres, 0, FAT_TIME, FAT_DATE, FAT_DATE,
                       cluster >> 16, FAT_TIME, FAT_DATE, cluster & 0xffff, size)


class Fat16:
    def __init__(self):
        self.fat = [0] * (CLUSTERS + 2)
        self.fat[0], self.fat[1] = 0xfff8, 0xffff
        self.data = bytearray(CLUSTERS * SPC * SECTOR)
        self.root = bytearray(ROOT_SECTORS * SECTOR)
        self.next = 2

    def alloc(self, nbytes):
        """Reserve contiguous clusters for nbytes; return the first one."""
        n = max(1, -(-nbytes // (SPC * SECTOR)))
        first = self.next
        for c in range(first, first + n):
            self.fat[c] = c + 1
        self.fat[first + n - 1] = 0xffff
        self.next += n
        if self.next > CLUSTERS + 2:
            raise SystemExit("image partition too small")
        return first

    def write(self, cluster, payload):
        off = (cluster - 2) * SPC * SECTOR
        self.data[off:off + len(payload)] = payload

    def mkdir(self, parent, entries):
        """Make a directory cluster holding '.', '..' and entries."""
        c = self.alloc(SPC * SECTOR)
        body = dirent(b".          ", 0x10, c, 0) + dirent(b"..         ", 0x10, parent, 0)
        self.write(c, body + b"".join(entries))
        return c

    def boot_sector(self):
        bs = bytearray(SECTOR)
        bs[0:3] = b"\xeb\x3c\x90"
        bs[3:11] = b"TEENYOS "
        struct.pack_into("<HBHBHHBHHHII", bs, 11, SECTOR, SPC, RESERVED, FATS, ROOT_ENTRIES,
                         PART_SECTORS if PART_SECTORS < 65536 else 0, 0xf8, FAT_SECTORS, 63, 255,
                         PART_START, 0 if PART_SECTORS < 65536 else PART_SECTORS)
        struct.pack_into("<BBBI11s8s", bs, 36, 0x80, 0, 0x29, 0x7e3d1f05, b"TEENYOS    ", b"FAT16   ")
        bs[510:512] = b"\x55\xaa"
        return bytes(bs)

    def image(self):
        fat = struct.pack(f"<{len(self.fat)}H", *self.fat)
        fat = fat.ljust(FAT_SECTORS * SECTOR, b"\0")
        out = bytearray(self.boot_sector())
        out += b"\0" * ((RESERVED - 1) * SECTOR)
        out += fat * FATS
        out += self.root
        out += self.data
        return bytes(out).ljust(PART_SECTORS * SECTOR, b"\0")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--loader", required=True)
    ap.add_argument("--kernel", required=True)
    ap.add_argument("-o", "--output", required=True)
    a = ap.parse_args()

    loader = open(a.loader, "rb").read()
    kernel = open(a.kernel, "rb").read()
    assert len(loader) == 16 * SECTOR and loader[512:520] == b"TEENYLDR"

    fs = Fat16()
    # Allocation order: directories first, then the big contiguous kernel.
    efi = fs.alloc(SPC * SECTOR)
    boot = fs.alloc(SPC * SECTOR)
    teeny = fs.alloc(SPC * SECTOR)
    readme = fs.alloc(len(README))
    kcl = fs.alloc(len(kernel))
    fs.write(kcl, kernel)
    fs.write(readme, README)
    fs.write(efi, dirent(b".          ", 0x10, efi, 0) + dirent(b"..         ", 0x10, 0, 0)
             + dirent(b"BOOT       ", 0x10, boot, 0))
    fs.write(boot, dirent(b".          ", 0x10, boot, 0) + dirent(b"..         ", 0x10, efi, 0)
             + dirent(b"BOOTX64 EFI", 0x20, kcl, len(kernel)))
    fs.write(teeny, dirent(b".          ", 0x10, teeny, 0) + dirent(b"..         ", 0x10, 0, 0)
             + dirent(b"README  TXT", 0x20, readme, len(README)))
    root = (dirent(b"TEENYOS    ", 0x08, 0, 0) + dirent(b"EFI        ", 0x10, efi, 0)
            + dirent(b"TEENYOS    ", 0x10, teeny, 0, lower=True))
    fs.root[:len(root)] = root

    # Tell stage 2 where the kernel is.
    kernel_lba = PART_START + DATA_START + (kcl - 2) * SPC
    kernel_sectors = -(-len(kernel) // SECTOR)
    stage2 = bytearray(loader[512:])
    struct.pack_into("<QI", stage2, 8, kernel_lba, kernel_sectors)

    mbr = bytearray(loader[:512])
    struct.pack_into("<IH", mbr, 440, 0x7e3d1f05, 0)
    # One bootable partition, type 0x0e (FAT16, LBA addressed).
    mbr[446:462] = struct.pack("<B3sB3sII", 0x80, b"\xfe\xff\xff", 0x0e, b"\xfe\xff\xff",
                               PART_START, PART_SECTORS)
    mbr[462:510] = b"\0" * 48
    mbr[510:512] = b"\x55\xaa"

    disk = bytearray(PART_START * SECTOR)
    disk[0:512] = mbr
    disk[512:512 + len(stage2)] = stage2
    disk += fs.image()
    open(a.output, "wb").write(disk)


if __name__ == "__main__":
    main()
