# teenyOS

A complete Linux system in one 754,176-byte file. Like Tiny Core Linux, it boots
to a shell, runs entirely from RAM, and comes with the usual base commands. Tiny
Core's smallest edition (Core, no GUI) is about 17 MB. teenyOS fits in 0.75 MB,
with about 240 KB of the 1 MB budget left over.

```
$ make
teenyOS image: 754176 bytes (budget 1000000)
$ make run-serial
...
root@teeny:~# uname -a; free
Linux teeny 6.6.58 #1 Mon Oct 26 04:33:08 UTC 2026 i686
         total      used      free    shared     cache available
Mem:      2244K     1020K      876K        0K      348K      612K
```

## The numbers

| | |
|---|---|
| Image on disk (kernel + root filesystem) | **754,176 bytes** |
| Userland binary (all 52 commands) | 111,128 bytes, ~55 KB compressed |
| Smallest machine it runs on | **5 MB of RAM** (tested in QEMU) |
| Memory free after boot on that machine | ~880 KB |

The storage goal is met. The RAM goal (under 1 MB) is not, and no Linux system
can meet it on a PC. On a 5 MB machine, the first megabyte is reserved by
the kernel because PC BIOSes scribble on it. The running kernel then takes
another 1.5 MB (its code alone is 1 MB). Linux has no configuration that
gets under that floor. A sub-1 MB-RAM system would mean writing a new kernel,
and then it would no longer be Linux.

What the userland itself costs: init plus two idle shells use about 250 KB,
and 112 KB of that is program text they share.

## What's in it

**Kernel:** Linux 6.6.58 LTS, 32-bit, built from `tinyconfig` plus
[`kernel/teeny.config`](kernel/teeny.config). It includes the VGA text console,
PC keyboard, serial console, procfs, sysfs, devtmpfs, ramfs, and APM power-off.
The root filesystem is linked into the kernel image and the whole thing is
compressed once with xz.

Two small patches in [`kernel/patches`](kernel/patches):

1. `0001` fixes a boot crash on machines with less than ~9 MB of RAM. On
   non-PAE i386, `memory_map_top_down()` asks for a free 4 MB-aligned block
   and does not check whether it got one. When none exists, it frees physical
   address 0–4 MB, which is where the running kernel lives. Without this
   patch the minimum RAM is 9 MB.
2. `0002` stops ~700 debug lines about page tables from pushing the boot log
   out of the 16 KB `dmesg` buffer on small machines.

**Userland:** one static Rust binary, [`userland`](userland), with no
standard library and no libc. It makes raw `int 0x80` system calls. Like
BusyBox, it acts as whichever command it is invoked as:

| | |
|---|---|
| system | `init` `sh` `ps` `kill` `free` `df` `mount` `umount` `dmesg` `uname` `uptime` `date` `hostname` `sleep` `sync` `env` `which` `whoami` `clear` `reboot` `poweroff` `halt` |
| files | `ls` `cp` `mv` `rm` `mkdir` `rmdir` `ln` `touch` `pwd` `chmod` `stat` |
| text | `cat` `echo` `wc` `head` `tail` `grep` `sort` `uniq` `tee` `seq` `more` `hexdump` `basename` `dirname` `test`/`[` `true` `false` |
| editor | `edit` (full-screen: arrows, ^S save, ^Q quit, ^F find, ^K/^U cut/paste line, ^G go to line) |

The shell supports quoting, `$VAR`, `${VAR:-default}`, `$(cmd)`, `$((arithmetic))`,
`*`/`?` globs, pipes, `> >> < 2> 2>&1`, `&& || ; &`,
`if/elif/else`, `while`, `until`, and `for` with `break`/`continue`, plus
`cd export unset set read shift source exec wait history`. Interactive use
adds line editing, history (up/down) and tab completion. `grep` takes
`. * ^ $` patterns and `a|b` with `-E`.

Boot sequence: `/init` mounts `/dev`, `/proc`, `/sys` and `/tmp`, sets the
hostname from `/etc/hostname`, runs `/etc/rc`, then keeps a login shell open
on `tty1` (the screen) and `ttyS0` (serial). A login shell reads
`/etc/profile` and `~/.profile`. Customize the system by editing
[`rootfs/`](rootfs) and rebuilding.

## Compared to Tiny Core

| Tiny Core has | teenyOS | Why |
|---|---|---|
| Shell and base commands | Yes | |
| Runs from RAM | Yes | |
| Networking | No | The IPv4 stack plus a NIC driver adds 440 KB to the image (measured), which breaks the budget. |
| Disk access and persistence | No | Block layer, ext2 and an ATA driver add 260 KB (measured). |
| GUI (X/FLTK) | No | X alone is larger than the entire budget. |
| Package manager | No | Nothing to download extensions over, and nothing to store them on. |

Everything you create lives in RAM and is gone after a reboot.

## Building

You need a Rust toolchain with the i686 musl target, a C toolchain that can
build the kernel, and QEMU to run it. On Ubuntu/Debian:

```sh
sudo apt install build-essential flex bison bc libelf-dev curl xz-utils patch qemu-system-x86
rustup target add i686-unknown-linux-musl
make            # downloads Linux 6.6.58 once (checksum-verified), then builds
```

`make` fails if the image goes over 1,000,000 bytes, and
`scripts/check_config.sh` fails the build if any kernel option in
`teeny.config` gets dropped by Kconfig.

| | |
|---|---|
| `make run` | Boot in a QEMU window (VGA console). `MEM=5M make run` to try the minimum. |
| `make run-serial` | Boot on your terminal. Quit QEMU with `Ctrl-A x`. |
| `make test` | Boot headless and run 32 checks: shell syntax, file commands, pipes, background jobs, Ctrl-C, the editor, tab completion. |
| `make clean` | Remove build output but keep the downloaded kernel tarball. |

## Booting real hardware

`build/teenyOS.bzImage` is a standard Linux kernel image, so any BIOS
bootloader that can load Linux will boot it. With GRUB:

```
menuentry "teenyOS" {
    linux /boot/teenyOS.bzImage
}
```

It needs an i686-class CPU or newer, at least 5 MB of RAM, and a PC keyboard
(PS/2, or USB with the BIOS's legacy emulation). There is no EFI stub, so a
UEFI-only machine needs CSM/legacy boot enabled. `poweroff` turns the
machine off through APM where the BIOS supports it (QEMU does). Elsewhere it
halts and you press the power button.

## Layout

```
kernel/teeny.config        kernel options on top of tinyconfig
kernel/patches/            the two kernel patches
userland/src/main.rs       command table and dispatch
userland/src/sh.rs         the shell
userland/src/edit.rs       the editor
userland/src/{files,text,system,init}.rs   the other commands
userland/src/{sys,rt,util}.rs              syscalls, startup, heap, helpers
rootfs/                    files copied into / (etc/rc, etc/profile, motd)
scripts/gen_initramfs.sh   builds the initramfs file list
scripts/smoke_test.py      the QEMU test
```
