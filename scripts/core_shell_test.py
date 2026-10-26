#!/usr/bin/env python3
"""End-to-end test of the teenyOS core shell, on real firmware.

Builds the core with the `selftest` feature, boots it under OVMF, and lets its
boot-time harness drive the real parser and executor -- pipes, command
substitution, variable expansion, `&&`/`||`, quoting, redirection and globbing
-- writing labelled output to /root/RESULTS. Persistence flushes that to the
FAT volume; we then read it back with mtools and check every case.

No keystrokes are injected: the shell runs the same code a user's commands hit,
and the archive on disk is the proof it worked.

usage: core_shell_test.py
"""
import os
import struct
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
TOP = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import vm as vmlib  # noqa: E402

EFI = os.path.join(TOP, "core/target/x86_64-unknown-uefi/release/teenycore.efi")

EXPECT = [
    ("pipe", "3"),
    ("pipe-sort", "5"),
    ("subst", "1 2 3"),
    ("subst-nested", "1 2"),
    ("var", "hi-hi"),
    ("status-false", "1"),
    ("status-true", "0"),
    ("and", "A"),
    ("or", "B"),
    ("and-skip", "D"),
    ("quote-dq", "a b c"),
    ("quote-sq", "x  y"),
    ("glob-star", "a.txt b.txt"),
    ("glob-class", "/root/g/a.txt /root/g/b.txt"),
    ("glob-none", "/root/g/*.zzz"),
]


def build():
    subprocess.run(["cargo", "build", "--release", "--features", "selftest"],
                   cwd=os.path.join(TOP, "core"), check=True)


def make_image(work):
    esp = os.path.join(work, "esp.img")
    subprocess.run(["dd", "if=/dev/zero", "of=" + esp, "bs=1M", "count=48"],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["mkfs.vfat", "-F", "32", esp], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["mmd", "-i", esp, "::/EFI", "::/EFI/BOOT"], check=True)
    subprocess.run(["mcopy", "-i", esp, EFI, "::/EFI/BOOT/BOOTX64.EFI"], check=True)
    return esp


def extract_results(esp, work):
    out = os.path.join(work, "home.dat")
    r = subprocess.run(["mcopy", "-i", esp, "::/teenyos/home.dat", out],
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit("could not read persistence archive: " + r.stderr.strip())
    data = open(out, "rb").read()
    if data[:8] != b"TEENYv1\n":
        raise SystemExit("bad archive magic %r" % data[:8])
    p, files = 8, {}
    while p < len(data):
        kind = data[p:p + 1]; p += 1
        (nl,) = struct.unpack_from("<I", data, p); p += 4
        rel = data[p:p + nl]; p += nl
        if kind == b"F":
            (dl,) = struct.unpack_from("<I", data, p); p += 4
            files[rel.decode()] = data[p:p + dl]; p += dl
        elif kind != b"D":
            break
    return files.get("RESULTS", b"").decode(errors="replace")


def section(text, name):
    body = text.split("=== %s ===" % name, 1)
    if len(body) < 2:
        return None
    rest = body[1]
    nxt = rest.find("=== ")
    return (rest if nxt < 0 else rest[:nxt]).strip()


def main():
    build()
    work = tempfile.mkdtemp(prefix="coretest")
    esp = make_image(work)
    print("booting core (selftest build) under OVMF...")
    v = vmlib.VM([esp], firmware="uefi", usb=False)
    try:
        time.sleep(20)  # boot + run harness + persist::save()
    finally:
        v.stop()

    text = extract_results(esp, work)
    if "=== DONE ===" not in text:
        print(text)
        raise SystemExit("selftest did not finish (no DONE marker)")

    ok = True
    for name, want in EXPECT:
        got = section(text, name)
        good = got == want
        if name == "and-skip" and got and "C" in got:
            good = False
        print(("ok  " if good else "BAD ") + "%-14s got=%r want=%r" % (name, got, want))
        ok = ok and good
    print("\nALL PASS" if ok else "\nFAILURES ABOVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
