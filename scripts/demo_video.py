#!/usr/bin/env python3
"""Record the ~10 second teenyOS demo video from a real boot in QEMU.

usage: demo_video.py USB_IMAGE OUT.mp4

A second QEMU monitor grabs frames continuously while the main thread
drives the desktop with the mouse and keyboard. Afterwards the boot is
sped up, captions are added, and ffmpeg encodes 1280x720 H.264.
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

IMAGE, OUT = sys.argv[1], sys.argv[2]
W, H = 1280, 720
FPS = 30
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
work = tempfile.mkdtemp(prefix="teenydemo")
cap_sock = os.path.join(work, "cap.sock")
frames = []  # (real time, path) of full-size frames
marks = {}
stop = threading.Event()


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
            if os.path.exists(path) and os.path.getsize(path) >= W * H * 3:
                break
            time.sleep(0.005)
        # Firmware screens before the desktop can be a different size.
        if os.path.exists(path) and os.path.getsize(path) >= W * H * 3:
            frames.append((t, path))
        n += 1


def mark(name):
    marks[name] = time.time()


stick = os.path.join(work, "stick.img")
shutil.copy(IMAGE, stick)
os.truncate(stick, 64 << 20)
t0 = time.time()
vm = VM([stick], firmware="uefi",
        extra=["-vga", "none", "-device", f"VGA,edid=on,xres={W},yres={H}",
               "-monitor", f"unix:{cap_sock},server,nowait"])
cap = threading.Thread(target=capture, daemon=True)
cap.start()
try:
    mark("start")
    assert vm.wait_serial("# ", 60), "did not boot"
    time.sleep(1.0)
    mark("desktop")
    vm.mouse_home(W, H)
    vm.type("uname -sr; free | head -n 2\n", delay=0.045)
    time.sleep(1.2)
    mark("files")
    vm.move(53, 128, steps=14, delay=0.015)
    vm.click(double=True)
    time.sleep(0.9)
    # Files opens in /root; go up to show the whole system.
    vm.move(375, 154, steps=10, delay=0.015)
    time.sleep(0.2)
    vm.click()
    time.sleep(0.9)
    mark("menu")
    vm.move(50, H - 17, steps=14, delay=0.015)
    vm.click()
    time.sleep(0.6)
    menu_top = H - 34 - (9 * 22 + 40) - 4
    vm.move(60, menu_top + 36 + 2 * 22 + 11, steps=8, delay=0.02)
    time.sleep(0.25)
    vm.click()
    time.sleep(1.0)
    mark("drag")
    # Drag the monitor window by its title bar towards the right.
    win_x, win_y = (W - 460) // 2 - 70 + 56, (H - 34 - 400) // 2 - 50 + 56
    vm.drag(win_x + 120, win_y + 12, win_x + 470, win_y + 40, steps=18, delay=0.02)
    time.sleep(0.8)
    mark("storage")
    vm.move(53, 296, steps=14, delay=0.015)
    vm.click(double=True)
    time.sleep(1.6)
    mark("end")
finally:
    stop.set()
    cap.join(timeout=5)
    vm.stop()

# Map output time to recorded time: boot fast, the rest close to real time.
# The firmware's own screens are skipped: the video opens as Linux takes
# over, a moment before the desktop paints.
desk = marks["desktop"]
segments = [
    (desk - 1.6, desk, 0.9),   # (real start, real end, output seconds)
    (desk, marks["end"], (marks["end"] - desk) / 1.75),
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
end_card = 1.7


def text(t, size, y, a, b, color="white", box=True):
    t = t.replace(":", r"\:").replace("'", r"\'")
    bx = ":box=1:boxcolor=black@0.55:boxborderw=18" if box else ""
    return (f"drawtext=fontfile={FONT}:text='{t}':fontsize={size}:fontcolor={color}{bx}"
            f":x=(w-text_w)/2:y={y}:enable='between(t,{a:.2f},{b:.2f})'")


boot_end = segments[0][2]
captions = ",".join([
    text("teenyOS: boots straight from a USB stick", 38, "34", 0.0, boot_end + 1.2),
    text("a whole desktop OS in 2 MB", 38, "34", boot_end + 1.2, boot_end + 3.6),
    text("terminal  ·  files  ·  network  ·  saves to your stick", 34, "34", boot_end + 3.6, body),
])
card = ",".join([
    text("teenyOS", 110, "(h/2)-150", 0, 99, "0x6fc3d0", box=False),
    text("2 MB  ·  BIOS + UEFI  ·  persistence  ·  Rust", 40, "(h/2)+10", 0, 99, box=False),
    text("github.com/mokshshah18/teenyOS", 32, "(h/2)+80", 0, 99, "0xb0b8c8", box=False),
])
filt = (f"[0:v]scale={W}:{H},fps={FPS},{captions},format=yuv420p[a];"
        f"color=c=0x16202c:s={W}x{H}:d={end_card}:r={FPS},{card},format=yuv420p[b];"
        f"[a][b]concat=n=2:v=1[v]")
subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", os.path.join(seq, "s%05d.ppm"),
                "-filter_complex", filt, "-map", "[v]", "-c:v", "libx264", "-preset", "slow", "-crf", "20",
                "-movflags", "+faststart", OUT], check=True)
print(f"{OUT}: {body + end_card:.1f}s, {len(frames)} frames captured in {time.time() - t0:.0f}s")
shutil.rmtree(work, ignore_errors=True)
