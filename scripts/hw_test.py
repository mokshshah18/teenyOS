#!/usr/bin/env python3
"""Real-hardware paths, tested in QEMU: boot the USB image under BIOS and
UEFI, the desktop, persistence, both kinds of install, and networking.

usage: hw_test.py USB_IMAGE [TEST...]
Needs qemu-system-x86_64, OVMF, sfdisk, mkfs.vfat and mtools.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import functools
import http.server

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vm import VM, pixel  # noqa: E402

IMAGE = sys.argv[1]
TMP = tempfile.mkdtemp(prefix="teenyhw")
results = []


def check(name):
    def wrap(fn):
        def run():
            t = time.time()
            try:
                fn()
                results.append((name, None))
                print(f"ok   {name} ({time.time() - t:.0f}s)", flush=True)
            except Exception as e:  # report and carry on with the next test
                results.append((name, e))
                print(f"FAIL {name}: {e!r}", flush=True)
                import traceback; traceback.print_exc()
        return run
    return wrap


def stick(size="256M"):
    path = os.path.join(TMP, f"stick{len(results)}.img")
    shutil.copy(IMAGE, path)
    os.truncate(path, int(size[:-1]) << 20)
    return path


def booted(vm, timeout=60):
    if not vm.wait_serial("# ", timeout):
        vm.screenshot(os.path.join(TMP, "fail.png"))
        raise AssertionError("no shell prompt on the serial console")


def answer(vm, cmd, replies, done, timeout=60):
    """Run an interactive command: wait for each prompt, send the reply."""
    start = len(vm.serial_text())
    vm.send_serial(cmd + "\r")
    for prompt, reply in replies:
        assert vm.wait_serial(prompt, 30, start), vm.serial_text()[start:][-400:]
        vm.send_serial(reply + "\r")
    assert vm.wait_serial(done, timeout, start), vm.serial_text()[start:][-400:]
    return vm.serial_text()[start:]


def poweroff(vm):
    vm.send_serial("poweroff\r")
    assert vm.wait_exit(30), "poweroff did not turn the machine off"


@check("UEFI boot from USB stick reaches the desktop")
def uefi_desktop():
    vm = VM([stick()], firmware="uefi")
    try:
        booted(vm)
        time.sleep(2)
        shot = vm.screenshot()
        w, h, _ = shot
        # The taskbar is near-black, the wallpaper teal.
        assert pixel(shot, w // 2, h - 10)[0] < 40, "no taskbar"
        assert pixel(shot, w - 5, h // 2)[2] > 60, "no wallpaper"
        out = vm.sh("ls /dev/pts; persist status")
        assert "Booted from: sd" in out, out
        poweroff(vm)
    finally:
        vm.stop()


@check("BIOS boot from USB stick reaches the desktop")
def bios_desktop():
    vm = VM([stick()], firmware="bios")
    try:
        booted(vm)
        out = vm.sh("cat /sys/class/graphics/fb0/virtual_size; uname -m")
        assert "1280,1024" in out and "x86_64" in out, out
        poweroff(vm)
    finally:
        vm.stop()


@check("persistence: enable on the stick, survives a reboot (UEFI -> BIOS)")
def persistence():
    s = stick()
    vm = VM([s], firmware="uefi")
    try:
        booted(vm)
        out = answer(vm, "persist enable", [("[y/N]", "y")], "Persistence is on")
        assert "sda2" in out or "sdb2" in out, out
        vm.sh("echo kept > ~/kept.txt")
        poweroff(vm)
    finally:
        vm.stop()
    vm = VM([s], firmware="bios")
    try:
        booted(vm)
        out = vm.sh("cat ~/kept.txt; persist status")
        assert "kept" in out and "on" in out, out
        out = vm.sh("persist disable")
        assert "off" in out, out
        poweroff(vm)
    finally:
        vm.stop()
    vm = VM([s], firmware="uefi")
    try:
        booted(vm)
        out = vm.sh("persist status; ls ~")
        assert "off" in out and "kept" not in out, out
        out = vm.sh("persist enable; cat ~/kept.txt")
        assert "saved before" in out and "kept" in out, out
        poweroff(vm)
    finally:
        vm.stop()


@check("install to a whole disk, boots on BIOS and UEFI")
def whole_disk():
    disk = os.path.join(TMP, "blank.img")
    open(disk, "wb").truncate(256 << 20)
    vm = VM([stick("16M"), disk], firmware="bios")
    try:
        booted(vm)
        listing = vm.sh("teeny-install")
        target = [l.split()[0] for l in listing.splitlines() if "256M" in l][0]
        vm.sh("echo notes > ~/notes.txt")
        answer(vm, f"teeny-install {target}", [("Type yes", "yes")], "Done.")
        poweroff(vm)
    finally:
        vm.stop()
    for fw in ("bios", "uefi"):
        vm = VM([disk], firmware=fw, usb=False)
        try:
            booted(vm)
            out = vm.sh("cat ~/notes.txt; persist status")
            assert "notes" in out and "on" in out, (fw, out)
            poweroff(vm)
        finally:
            vm.stop()


@check("install alongside an existing OS via the UEFI boot menu")
def alongside():
    pc = os.path.join(TMP, "pc.img")
    open(pc, "wb").truncate(512 << 20)
    subprocess.run(["sfdisk", "-q", pc], check=True, input=(
        'label: gpt\nsize=100MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n'
        'type=EBD0A0A2-B9E5-4433-87C0-68B6B72699C7\n').encode())
    subprocess.run(["mkfs.vfat", "-F", "32", "-s", "1", "--offset", "2048", pc, "102400"],
                   check=True, capture_output=True)
    other = os.path.join(TMP, "bootmgfw.efi")
    open(other, "w").write("another OS\n")
    subprocess.run(["mmd", "-i", pc + "@@1M", "::/EFI", "::/EFI/Other"], check=True)
    subprocess.run(["mcopy", "-i", pc + "@@1M", other, "::/EFI/Other/"], check=True)
    nvram = os.path.join(TMP, "vars.fd")
    # No bootindex: with one, OVMF prunes boot entries QEMU didn't list.
    vm = VM([stick("16M"), pc], firmware="uefi", vars=nvram, bootindex=False)
    try:
        booted(vm)
        vm.sh("echo carried > ~/carried.txt")
        answer(vm, "teeny-install --alongside", [("Type yes", "yes"), ("[y/N]", "y")], "Done.")
        poweroff(vm)
    finally:
        vm.stop()
    vm = VM([pc], firmware="uefi", usb=False, vars=nvram, bootindex=False)
    try:
        booted(vm)
        out = vm.sh("cat ~/carried.txt; persist status")
        assert "carried" in out and "on" in out, out
        poweroff(vm)
    finally:
        vm.stop()
    listing = subprocess.run(["mdir", "-i", pc + "@@1M", "-/", "::EFI"], capture_output=True, text=True).stdout
    assert "bootmgfw" in listing, "the other OS's files were touched"


@check("networking: DHCP, ping, wget, share")
def networking():
    root = os.path.join(TMP, "www")
    os.makedirs(root)
    open(os.path.join(root, "hello.txt"), "w").write("hello over http\n")
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *a):
            pass
    handler = functools.partial(Quiet, directory=root)
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    vm = VM([stick("16M")], firmware="uefi",
            extra=["-netdev", "user,id=n0,hostfwd=tcp:127.0.0.1:18081-:8080", "-device", "e1000,netdev=n0"])
    try:
        booted(vm)
        for _ in range(30):
            if "Online" in vm.sh("net"):
                break
            time.sleep(1)
        out = vm.sh("net; ping -c 2 10.0.2.2")
        assert "10.0.2.15" in out and "reply from" in out, out
        out = vm.sh(f"cd /tmp && wget http://10.0.2.2:{port}/hello.txt && cat hello.txt")
        assert "hello over http" in out, out
        vm.sh("echo from-teeny > ~/f.txt; share /root 8080 > /dev/null &")
        time.sleep(1)
        got = subprocess.run(["curl", "-s", "--noproxy", "*", "http://127.0.0.1:18081/f.txt"],
                             capture_output=True, text=True).stdout
        assert got.strip() == "from-teeny", got
        poweroff(vm)
    finally:
        vm.stop()
        srv.shutdown()


for test in [globals()[n] for n in (sys.argv[2:] or ["uefi_desktop", "bios_desktop", "persistence", "whole_disk", "alongside", "networking"])]:
    test()
shutil.rmtree(TMP, ignore_errors=True)
failed = [n for n, e in results if e]
print(f"{len(results) - len(failed)}/{len(results)} hardware tests passed")
sys.exit(1 if failed else 0)
