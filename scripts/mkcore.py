#!/usr/bin/env python3
"""Build a bootable teenyOS core image.

usage: mkcore.py CORE.efi OUT.img [size_mb]

The image is one FAT32 EFI System Partition with the core at the removable-
media default path, \\EFI\\BOOT\\BOOTX64.EFI. Any UEFI machine -- a PC or a USB
stick plugged into one -- boots it with no boot entry to install. The volume is
writable, so `persist save` keeps /root on the same stick (under \\teenyos).

Uses mtools, so it needs neither root nor a loopback mount.
"""
import os
import subprocess
import sys

efi, out = sys.argv[1], sys.argv[2]
size_mb = int(sys.argv[3]) if len(sys.argv) > 3 else 64


def run(*cmd):
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)


run("dd", "if=/dev/zero", "of=" + out, "bs=1M", "count=%d" % size_mb)
run("mkfs.vfat", "-F", "32", "-n", "TEENYOS", out)
run("mmd", "-i", out, "::/EFI", "::/EFI/BOOT")
run("mcopy", "-i", out, efi, "::/EFI/BOOT/BOOTX64.EFI")

print("core image: %s (%d bytes)" % (out, os.path.getsize(out)))
print("  boot in QEMU:  make run-core")
print("  write to USB:  dd if=%s of=/dev/sdX bs=4M  (or Rufus/Etcher)" % out)
