#!/usr/bin/env python3
"""Record a ~12 second teenyOS core demo from a real boot in QEMU/OVMF.

usage: core_demo.py CORE.img OUT.mp4   (defaults: build/teenyOS-core.img, docs/teenyOS-core-demo.mp4)

Core renders to the firmware framebuffer, not the serial line, so this times
the boot rather than waiting on a prompt, and drives the desktop with the
keyboard and the window-manager function keys. A second QEMU monitor grabs
frames the whole time; afterwards the boot is sped up, captions go on, and
ffmpeg encodes 1280x720 H.264.
"""
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vm import VM  # noqa: E402

TOP = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGE = sys.argv[1] if len(sys.argv) > 1 else os.path.join(TOP, "build/teenyOS-core.img")
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(TOP, "docs/teenyOS-core-demo.mp4")
W, H, FPS = 1280, 720, 30
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
work = tempfile.mkdtemp(prefix="coredemo")
cap_sock = os.path.join(work, "cap.sock")
frames = []  # (time, path, w, h)
marks = {}
stop = threading.Event()


def ppm_size(path):
    try:
        with open(path, "rb") as f:
            head = f.read(64)
        if not head.startswith(b"P6"):
            return None
        parts = head.split()
        return int(parts[1]), int(parts[2])
    except Exception:
        return None


def capture():
    s = socket.socket(socket.AF_UNIX)
    for _ in range(100):
        try:
            s.connect(cap_sock)
            break
        except OSError:
            time.sleep(0.05)
    s.setblocking(False)
    n = 0
    while not stop.is_set():
        path = os.path.join(work, f"f{n:05d}.ppm")
        t = time.time()
        s.sendall(f"screendump {path}\n".encode())
        for _ in range(200):
            try:
                while s.recv(65536):
                    pass
            except BlockingIOError:
                pass
            sz = ppm_size(path)
            if sz and os.path.getsize(path) >= sz[0] * sz[1] * 3:
                frames.append((t, path, sz[0], sz[1]))
                break
            time.sleep(0.005)
        n += 1


def mark(name):
    marks[name] = time.time()


img = os.path.join(work, "core.img")
shutil.copy(IMAGE, img)
t0 = time.time()
vm = VM([img], firmware="uefi", usb=False, mem="128M",
        extra=["-vga", "none", "-device", f"VGA,edid=on,xres={W},yres={H}",
               "-monitor", f"unix:{cap_sock},server,nowait"])
cap = threading.Thread(target=capture, daemon=True)
cap.start()
try:
    mark("start")
    time.sleep(7.0)  # OVMF + core boot to the desktop
    mark("desktop")
    vm.mouse_home(W, H)
    # First terminal: prove it's a real shell on the firmware.
    vm.type("uname -a\n", delay=0.04)
    time.sleep(0.6)
    vm.type("seq 5 | sort -r | head -3\n", delay=0.04)
    time.sleep(0.8)
    vm.type("echo teenyOS > hi.txt\n", delay=0.04)
    vm.type("cat hi.txt\n", delay=0.04)
    time.sleep(0.8)
    mark("second")
    vm.key("f1")            # open a second terminal window
    time.sleep(0.8)
    vm.type("free\n", delay=0.04)
    time.sleep(0.7)
    vm.type("net\n", delay=0.04)
    time.sleep(1.0)
    mark("persist")
    vm.type("persist save\n", delay=0.04)
    time.sleep(1.2)
    mark("end")
finally:
    stop.set()
    cap.join(timeout=5)
    vm.stop()

if not frames:
    raise SystemExit("no frames captured")
# Keep only frames at the dominant (desktop) resolution.
from collections import Counter  # noqa: E402
cw, ch = Counter((w, h) for _, _, w, h in frames).most_common(1)[0][0]
frames = [(t, p) for (t, p, w, h) in frames if (w, h) == (cw, ch)]
print(f"captured {len(frames)} desktop frames at {cw}x{ch}")

desk = marks["desktop"]
segments = [
    (desk - 1.2, desk, 0.8),                       # a beat of the fresh desktop
    (desk, marks["end"], (marks["end"] - desk) / 1.5),
]
out_frames = []
for (a, b, dur) in segments:
    n = max(1, int(dur * FPS))
    for i in range(n):
        out_frames.append(a + (b - a) * i / n)
seq = os.path.join(work, "seq")
os.makedirs(seq)
j = 0
for k, t in enumerate(out_frames):
    while j + 1 < len(frames) and frames[j + 1][0] <= t:
        j += 1
    os.symlink(frames[j][1], os.path.join(seq, f"s{k:05d}.ppm"))
body = len(out_frames) / FPS
end_card = 1.8


def text(t, size, y, a, b, color="white", box=True):
    t = t.replace(":", r"\:").replace("'", r"\'")
    bx = ":box=1:boxcolor=black@0.55:boxborderw=18" if box else ""
    return (f"drawtext=fontfile={FONT}:text='{t}':fontsize={size}:fontcolor={color}{bx}"
            f":x=(w-text_w)/2:y={y}:enable='between(t,{a:.2f},{b:.2f})'")


boot_end = segments[0][2]
captions = ",".join([
    text("teenyOS core: a desktop OS with no kernel", 38, "34", 0.0, boot_end + 2.2),
    text("121 KB, running on the UEFI firmware itself", 38, "34", boot_end + 2.2, boot_end + 4.6),
    text("real shell  ·  pipes  ·  files  ·  networking", 34, "34", boot_end + 4.6, body),
])
card = ",".join([
    text("teenyOS core", 100, "(h/2)-150", 0, 99, "0x6fc3d0", box=False),
    text("121 KB  ·  no kernel  ·  UEFI  ·  Rust", 40, "(h/2)+10", 0, 99, box=False),
    text("github.com/mokshshah18/teenyOS", 32, "(h/2)+80", 0, 99, "0xb0b8c8", box=False),
])
filt = (f"[0:v]scale={W}:{H},fps={FPS},{captions},format=yuv420p[a];"
        f"color=c=0x16202c:s={W}x{H}:d={end_card}:r={FPS},{card},format=yuv420p[b];"
        f"[a][b]concat=n=2:v=1[v]")
os.makedirs(os.path.dirname(OUT), exist_ok=True)
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS),
                "-i", os.path.join(seq, "s%05d.ppm"), "-filter_complex", filt,
                "-map", "[v]", "-c:v", "libx264", "-preset", "slow", "-crf", "20",
                "-movflags", "+faststart", OUT], check=True)
print(f"{OUT}: {body + end_card:.1f}s, {len(frames)} frames captured in {time.time() - t0:.0f}s")
shutil.rmtree(work, ignore_errors=True)
