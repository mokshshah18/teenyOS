# teenyOS

A desktop operating system in a 2 MB file. It boots from a USB stick on BIOS and
UEFI PCs, runs from RAM, can keep your files on the stick, installs onto a PC,
and gets online over Ethernet or a phone's USB tethering. The kernel is
Linux; everything else (init, shell, desktop, tools) is one Rust program
with no libc.

Demo video (10 s, recorded from a real boot in QEMU): [docs/teenyOS-demo.mp4](docs/teenyOS-demo.mp4)

There are two editions. The one described first runs on the Linux kernel and
targets real, varied PC hardware. The second, **[teenyOS core](#teenyos-core--the-kernel-less-edition)**,
has no kernel at all: it is a 120 KB program that runs on the UEFI firmware and
still has a windowed desktop, a real shell, a filesystem, persistence and
networking. It is the answer to "do all of that in under a megabyte."

## Why 2 MB and not 1 MB

The first version of teenyOS was a 754 KB image. It booted in QEMU to a shell
and could not see a disk, a mouse or a network. Tiny Core Linux is called the
smallest *functional* OS because people can actually use it: it boots real
machines, has a desktop, keeps files across reboots and gets online. Measured
against that, a 754 KB image that does none of this is a gimmick. So the
target became Tiny Core's feature set in the smallest image that delivers it.
The console-only build still exists as `make mini` (877 KB).

What it costs to work on real PCs, measured on this kernel:

| Addition | Image grows by |
|---|---|
| Real-PC support: PCI, USB keyboards and sticks, SATA, NVMe, FAT, framebuffer, mouse, ACPI, UEFI boot stub | +680 KB |
| IPv4 networking + Intel, Realtek and USB-tethering drivers | +490 KB |

## The numbers

| | teenyOS | Tiny Core 16 (TinyCore edition) |
|---|---|---|
| Size | **2.06 MB** kernel file (image download: 2.0 MB compressed) | about 23 MB ISO |
| RAM to run the desktop | boots to it in 16 MB (32 MB comfortable) | 46 MB |
| Boots from USB on BIOS | yes (own 8 KB boot loader) | yes |
| Boots from USB on UEFI | yes (the kernel is the EFI app) | CorePure64 |
| Desktop | yes: windows, mouse, terminal, files, monitor | yes (X + FLWM) |
| Keeps files across reboots | yes, on the stick or a PC disk | yes |
| Installs to a PC | yes: whole disk, or alongside Windows/Linux | yes |
| Wired network + DHCP | yes, plus phone USB tethering | yes |
| Wi-Fi | **no** | via extensions |
| Package manager | **no** | yes (tce-load) |
| HTTPS | **no** (no TLS stack) | via extensions |

Tiny Core figures are from press coverage of release 16.2
([3DTested](https://www.3dtested.com/software/linux/tiny-core-linux-16-2-still-fits-a-proper-linux-desktop-into-a-23mb-download-but-it-has-grown-1mb-since-the-last-time-we-looked-at-it),
[Wikipedia](https://en.wikipedia.org/wiki/Tiny_Core_Linux)); tinycorelinux.net was down when this was written.

The three "no" rows are where teenyOS falls short of Tiny Core. Wi-Fi needs
megabytes of firmware blobs plus WPA. A package manager needs a package
repository, and Tiny Core's packages are built against its glibc. HTTPS
needs TLS.

## Put it on a USB stick

Build it (below) or take `teenyOS-usb.img`, then write it to a stick. This
erases the stick.

- Windows/macOS: [balenaEtcher](https://etcher.balena.io) or Rufus (DD mode)
- Linux: `sudo dd if=build/teenyOS-usb.img of=/dev/sdX bs=4M conv=fsync`

Plug it in, power on, and pick the stick from the boot menu (F12, F11, F8 or
Esc on most PCs). **Turn Secure Boot off**: the kernel is not signed.

## Using it

The desktop starts with a terminal open. Desktop icons open on double click,
and the **teenyOS** button opens the menu (apps, exit to console, reboot,
power off). Drag windows by their title bar. The taskbar shows network,
battery and storage status.

**Keeping your files (persistence).** Everything lives in RAM until you turn
persistence on, with the **Storage** app or `persist enable`. On a USB
stick this turns the stick's unused space into a FAT32 data partition
(`TEENYDATA`), copies `/root` into it, and restores it at every boot. On a
disk without free space it uses a `teenyos/data` folder on the boot
partition instead. `persist disable` goes back to RAM only and keeps the
saved files in `teenyos/data.off` for later.

**Installing on a PC.** From the Storage app or a terminal:

- `teeny-install --alongside` adds teenyOS to the PC's EFI system partition
  (one 2 MB folder) and to its UEFI boot menu, with persistence. Windows or
  your current Linux keeps working, and the installer asks whether teenyOS
  should become the default. This needs the stick booted in UEFI mode.
- `teeny-install sdb` erases a whole disk and makes it boot teenyOS on both
  BIOS and UEFI, with persistence. Run `teeny-install` alone to list disks.

**Network.** Plug in a cable, a Realtek or Intel USB Ethernet adapter, or a
phone with USB tethering turned on. DHCP runs automatically.

| | |
|---|---|
| `net` | interfaces, link, address, gateway, DNS |
| `ping example.com` | ICMP echo |
| `wget http://host/file` | download over HTTP (no HTTPS) |
| `share [DIR] [PORT]` | serve a folder to other devices: they open `http://<teenyOS address>:8080/` to download, or upload from the page |

**Commands.** 59 of them: `sh` (pipes, redirects, `$(...)`, `$((...))`,
globs, `if/while/for`, history, tab completion), `edit` (full-screen
editor), `ls cp mv rm mkdir ln chmod stat cat grep sort uniq head tail wc
tee seq more hexdump ps kill free df mount dmesg date uptime` and more. Run
`help` for the list.

## Hardware

x86_64 PCs with BIOS or UEFI. Keyboard and mouse can be
USB or PS/2; touchpads work in basic PS/2 mode. Disks can be USB, SATA or
NVMe. Display: whatever mode the firmware sets up (VESA up to 1280x1024
on BIOS, the native GOP mode on UEFI, drawn at 2x on screens 2400+ pixels
wide).

Everything has been tested in QEMU with SeaBIOS and OVMF (see
[Tests](#tests)), but not yet on physical machines. Real firmware is more
varied than QEMU's.

## Building

Ubuntu/Debian:

```sh
sudo apt install build-essential flex bison bc libelf-dev curl xz-utils patch \
    python3 qemu-system-x86 ovmf mtools dosfstools fdisk ffmpeg
rustup target add x86_64-unknown-linux-musl
make             # build/teenyOS.efi and build/teenyOS-usb.img
```

`make` downloads Linux 6.6.58 once (checksum-verified) and applies the
patches in `kernel/patches`. It fails if the kernel goes over its budget
(2.2 MB full, 1 MB mini) or if Kconfig silently drops any requested option.

| | |
|---|---|
| `make run` / `make run-uefi` | boot the USB image in QEMU on BIOS / UEFI |
| `make mini`, `make run-mini` | the 877 KB console-only build, on this terminal |
| `make test` | all automated tests (below) |
| `scripts/demo_video.py build/teenyOS-usb.img out.mp4` | re-record the demo video |

## Tests

`make test` runs two suites in QEMU:

- `scripts/smoke_test.py`: 32 checks of the shell and tools on the mini
  build. These cover pipes, loops, redirection, background jobs, Ctrl-C,
  the editor and tab completion.
- `scripts/hw_test.py` covers the real-hardware paths:
  - BIOS and UEFI boot from an emulated USB stick on xHCI, with the desktop
    up.
  - Persistence turned on under UEFI, the file back after a BIOS reboot,
    then off and back on.
  - Whole-disk install, booted on BIOS and on UEFI.
  - Install alongside a GPT disk with another OS. The PC then boots teenyOS
    from the new UEFI boot entry with the stick removed, and the other OS's
    files are untouched.
  - DHCP, `ping`, `wget` and `share`.

## How it works

- **Kernel:** Linux 6.6 LTS from `tinyconfig` plus `kernel/base.config` and
  `kernel/full.config`. The root filesystem is linked into the kernel and
  everything is xz-compressed once. With the EFI stub, the same file is
  both a bzImage and `BOOTX64.EFI`. Two patches:
  - `0001` stops a boot crash on machines too small for a 4 MB-aligned
    page-table block.
  - `0003` drops the 2 MB section alignment on x86_64, which halves the
    memory the decompressor reserves.
- **BIOS loader** (`boot/loader.S`, 8 KB): the MBR plus 15 sectors. It
  picks a VESA mode the kernel will accept, reads the kernel file from the
  FAT partition by its disk position, and starts it with the Linux 16-bit
  boot protocol. The installer finds that position with `FIBMAP` and
  patches it in.
- **Userland** (`userland/`): one static Rust binary that picks its role
  from `argv[0]`, the way BusyBox does. It uses raw syscalls and its own
  heap, with no std and no libc.
  - `desktop.rs`: the window manager.
  - `term.rs`: a VT100 terminal on a pty.
  - `fb.rs`: framebuffer drawing in the kernel's console font, read at
    runtime.
  - `persist.rs` and `install.rs`: persistence and installing.
  - `disk.rs`: partitioning and a FAT32 formatter.
  - `net.rs`: DHCP, DNS, ping, wget and share.
  - `sh.rs`: the shell.
- **Image** (`scripts/mkimage.py`): MBR, loader, and an 8 MB FAT16
  partition written in pure Python, with the kernel in contiguous clusters.

# teenyOS core — the kernel-less edition

The Linux edition stops at 877 KB because a kernel that can see real hardware
costs most of that. teenyOS core removes the kernel. It is a **121 KB** UEFI
application (`core/`) that boots to a windowed desktop and never looks back —
and the whole OS fits in that one figure, because it writes none of the drivers
an OS usually carries.

## Firmware as the driver layer

A UEFI machine, before an OS starts, already has working drivers: the graphics
output protocol (GOP) is drawing to the panel, a pointer protocol is tracking
the mouse, the keyboard protocol is reading keys, a FAT driver is mounted on
the boot volume, and — on most firmware — a full network stack sits behind
`EFI_HTTP_PROTOCOL`, DHCP and DNS included. An OS normally throws all of that
away the instant it calls `ExitBootServices` and starts talking to the chips
itself.

teenyOS core simply never makes that call. It stays a UEFI application with boot
services live, and calls the firmware's protocols the way a program calls a
library. So the code it does *not* contain is the code that makes other kernels
big: no GOP driver, no USB stack, no NIC driver, no TLS implementation, no FAT
code. The firmware already has them. What teenyOS core adds on top is only the
parts that make it feel like an OS:

- `sched.rs`: cooperative green threads, so each terminal window is its own
  shell running concurrently on the single thread the firmware gave us.
- `vfs.rs`, `proc.rs`, `sysb.rs`: a RAM filesystem, per-thread file-descriptor
  tables, and a small POSIX-style syscall surface over them.
- `desktop.rs`, `fb.rs`, `tty.rs`: the window manager, the drawing primitives,
  and a VT100 terminal — painted through the firmware's GOP.
- `shell.rs`: the shell (below).
- `net.rs`, `persist.rs`: downloads and persistence, both over firmware
  protocols.
- `rt.rs`: a slab allocator over one firmware memory block, and the `#![no_std]`
  runtime — memory intrinsics, panic handler, no libc.

## What it does

- **Desktop.** Draggable windows, a taskbar, an About window, and as many
  terminal windows as you open, each a separate shell thread. `F1` opens a
  terminal, `F2` cycles focus, `F3` the About window, `F4` closes one.
- **Shell.** Quoting, `$VAR` / `${VAR}` / `$?` and `$(command)` substitution
  (nested), filename globbing (`*`, `?`, `[a-z]`), `;` `&&` `||` sequencing,
  `|` pipelines, `> >> < 2>` redirection, tab completion and history. Plus the
  applets shared with the Linux edition: `ls cp mv rm mkdir rmdir ln touch pwd
  chmod stat cat grep wc head tail sort uniq tee seq basename dirname hexdump
  more test clear uname date free ps env`.
- **Networking.** `net` reports what the firmware's stack offers; `wget
  http://host/path` downloads a file through `EFI_HTTP_PROTOCOL`, bringing the
  link up with DHCP (falling back to a static address).
- **Persistence.** The filesystem is in RAM; `persist save` packs `/root` into
  a single archive and writes it to the boot volume through the firmware's FAT
  driver, under `\teenyos\home.dat`. It is restored at the next boot. Power off
  from the menu saves first.

## HTTPS, honestly

`wget https://…` takes the same path as `http://`: `EFI_HTTP_PROTOCOL`
negotiates TLS itself when the firmware ships a TLS protocol, and this build
confirms that protocol is present before using it. So HTTPS works as far as the
firmware exposes it, and teenyOS core carries no TLS code of its own to make it
work. The honest caveat: in the OVMF/QEMU sandbox used for testing, the
firmware's certificate store has no public CAs and outbound traffic is proxied,
so a real fetch from a public HTTPS site is not something this environment can
demonstrate. The plaintext HTTP path is tested end to end (below); the HTTPS
path is the same code with the firmware's TLS switched on.

## The size story

The OS is the 121 KB `BOOTX64.EFI`. The bootable *image* (`make core`) is
larger — 64 MB by default — but that is just an empty FAT32 volume to hold the
binary and your saved files, the same way a 23 MB ISO is mostly empty space
around a kernel. Nothing in the 64 MB is teenyOS; it is the stick. Measure the
OS by the binary.

## Build, run, test

```sh
rustup target add x86_64-unknown-uefi
make core        # build/teenyOS-core.img, a bootable UEFI stick
make run-core    # boot it in QEMU under OVMF
make core-test   # end-to-end test of the shell on real firmware
```

Write `build/teenyOS-core.img` to a USB stick exactly like the Linux edition
(Etcher, Rufus in DD mode, or `dd`), plug it into any UEFI PC with Secure Boot
off, and it boots to the desktop. The stick is writable, so `persist save`
keeps your files on it.

`make core-test` is unusual and worth explaining. There is no way to type `ls |
sort | head` into a QEMU framebuffer reliably, so the test does not try. A
`selftest` build runs the real parser and executor at boot against a fixed
script — pipes, substitution, variables, `&&`/`||`, quoting, redirection,
globbing — writes the labelled output to `/root/RESULTS`, and persists it. The
host then reads that archive back off the FAT image and checks every case. The
shell under test is the exact code a user's keystrokes reach; the file on disk
is the proof it ran.
