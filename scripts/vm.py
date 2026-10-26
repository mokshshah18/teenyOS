"""Drive teenyOS in QEMU: boot it, type, move and click the mouse, take
screenshots, read the serial console. Used by the tests and the demo video.
"""
import os
import select
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import zlib

OVMF = "/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS = "/usr/share/OVMF/OVMF_VARS_4M.fd"

KEYS = {
    " ": "spc", "\n": "ret", "\r": "ret", "\t": "tab", "-": "minus", "=": "equal",
    "/": "slash", "\\": "backslash", ".": "dot", ",": "comma", ";": "semicolon",
    "'": "apostrophe", "`": "grave_accent", "[": "bracket_left", "]": "bracket_right",
    "|": "shift-backslash", "$": "shift-4", "(": "shift-9", ")": "shift-0",
    "*": "shift-8", ">": "shift-dot", "<": "shift-comma", '"': "shift-apostrophe",
    "&": "shift-7", "!": "shift-1", "_": "shift-minus", ":": "shift-semicolon",
    "~": "shift-grave_accent", "?": "shift-slash", "+": "shift-equal", "#": "shift-3",
    "{": "shift-bracket_left", "}": "shift-bracket_right", "@": "shift-2", "%": "shift-5",
}


class VM:
    def __init__(self, disks, firmware="uefi", mem="256M", usb=True, extra=(), kernel=None, append=None, vars=None, bootindex=True):
        """disks: list of raw image paths. The first is attached as a USB
        stick when usb=True, otherwise as an IDE disk; the rest are SATA."""
        self.dir = tempfile.mkdtemp(prefix="teenyvm")
        self.mon_path = os.path.join(self.dir, "mon.sock")
        self.ser_path = os.path.join(self.dir, "serial.sock")
        self.ser_buf = b""
        args = ["qemu-system-x86_64", "-m", mem, "-display", "none", "-no-reboot",
                "-monitor", f"unix:{self.mon_path},server,nowait",
                "-serial", f"unix:{self.ser_path},server,nowait"]
        if firmware == "uefi":
            # Pass `vars` to keep UEFI settings (boot entries) between runs.
            vars_copy = vars or os.path.join(self.dir, "vars.fd")
            if not os.path.exists(vars_copy):
                shutil.copy(OVMF_VARS, vars_copy)
            args += ["-drive", f"if=pflash,format=raw,readonly=on,file={OVMF}",
                     "-drive", f"if=pflash,format=raw,file={vars_copy}"]
        if kernel:
            args += ["-kernel", kernel]
            if append:
                args += ["-append", append]
        args += ["-device", "qemu-xhci,id=xhci", "-device", "ahci,id=ahci"]
        for i, d in enumerate(disks):
            args += ["-drive", f"file={d},format=raw,if=none,id=d{i}"]
            # With an explicit bootindex OVMF rewrites BootOrder to match
            # QEMU's list, dropping entries an OS added; pass bootindex=False
            # to test boot entries.
            bi = ",bootindex=0" if bootindex else ""
            if i == 0 and usb:
                args += ["-device", f"usb-storage,bus=xhci.0,drive=d{i}{bi}"]
            elif i == 0:
                args += ["-device", f"ide-hd,drive=d{i}{bi}"]
            else:
                args += ["-device", f"ide-hd,bus=ahci.{i - 1},drive=d{i}"]
        args += list(extra)
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                self.mon = socket.socket(socket.AF_UNIX)
                self.mon.connect(self.mon_path)
                break
            except OSError:
                time.sleep(0.05)
        self.mon.setblocking(False)
        # QEMU creates the sockets one after another; wait for this one too.
        for _ in range(100):
            try:
                self.ser = socket.socket(socket.AF_UNIX)
                self.ser.connect(self.ser_path)
                break
            except OSError:
                time.sleep(0.05)
        self.ser.setblocking(False)
        self.mx, self.my = None, None

    def cmd(self, c):
        self.mon.sendall((c + "\n").encode())
        time.sleep(0.02)
        try:
            while self.mon.recv(65536):
                pass
        except BlockingIOError:
            pass

    def serial_text(self):
        try:
            while True:
                d = self.ser.recv(65536)
                if not d:
                    break
                self.ser_buf += d
        except (BlockingIOError, OSError):
            pass
        return self.ser_buf.decode(errors="replace")

    def wait_serial(self, text, timeout=30, start=0):
        """Wait until `text` appears in serial output after offset `start`."""
        end = time.time() + timeout
        while time.time() < end:
            if text in self.serial_text()[start:]:
                return True
            time.sleep(0.1)
        return False

    def sh(self, line, timeout=30):
        """Run a command in the serial-console shell and return its output."""
        start = len(self.serial_text())
        mark = "@@%d@@" % int(time.time() * 1000)
        cmd = line + "; echo " + mark[:2] + '""' + mark[2:]
        for ch in cmd + "\r":
            self.ser.sendall(ch.encode())
            time.sleep(0.004)
        if not self.wait_serial(mark, timeout, start):
            raise TimeoutError(f"no output from {line!r}: {self.serial_text()[start:][-500:]!r}")
        out = self.serial_text()[start:]
        out = out[out.find("\n") + 1:out.rfind(mark)]
        return out.replace("\r", "")

    def send_serial(self, text):
        for ch in text:
            self.ser.sendall(ch.encode())
            time.sleep(0.004)

    def key(self, k, delay=0.03):
        self.cmd("sendkey " + k)
        time.sleep(delay)

    def type(self, text, delay=0.03):
        for ch in text:
            k = KEYS.get(ch)
            if k is None:
                k = ("shift-" + ch.lower()) if ch.isupper() else ch
            self.key(k, delay)

    def mouse_home(self, w, h):
        """Assume the desktop just started: the pointer is at the centre."""
        self.mx, self.my = w // 2, h // 2

    def move(self, x, y, steps=1, delay=0.0):
        """Move the pointer to (x, y) in screen pixels, in `steps` hops."""
        for i in range(1, steps + 1):
            tx = self.mx + (x - self.mx) * 1 // (steps - i + 1)
            ty = self.my + (y - self.my) * 1 // (steps - i + 1)
            dx, dy = tx - self.mx, ty - self.my
            while dx or dy:
                sx, sy = max(-100, min(100, dx)), max(-100, min(100, dy))
                self.cmd(f"mouse_move {sx} {sy}")
                dx -= sx
                dy -= sy
            self.mx, self.my = tx, ty
            time.sleep(delay)

    def click(self, x=None, y=None, double=False):
        if x is not None:
            self.move(x, y)
            time.sleep(0.1)
        for _ in range(2 if double else 1):
            self.cmd("mouse_button 1")
            time.sleep(0.05)
            self.cmd("mouse_button 0")
            time.sleep(0.08)

    def drag(self, x0, y0, x1, y1, steps=10, delay=0.03):
        self.move(x0, y0)
        time.sleep(0.1)
        self.cmd("mouse_button 1")
        time.sleep(0.05)
        self.move(x1, y1, steps, delay)
        self.cmd("mouse_button 0")

    def screenshot(self, png=None):
        """Grab the screen; returns (w, h, rgb bytes) and optionally writes a PNG."""
        ppm = os.path.join(self.dir, "shot.ppm")
        if os.path.exists(ppm):
            os.remove(ppm)
        self.cmd(f"screendump {ppm}")
        for _ in range(100):
            if os.path.exists(ppm) and os.path.getsize(ppm) > 100:
                d = open(ppm, "rb").read()
                parts = d.split(b"\n", 3)
                w, h = map(int, parts[1].split())
                if len(parts[3]) >= w * h * 3:
                    break
            time.sleep(0.05)
        px = parts[3][:w * h * 3]
        if png:
            write_png(png, w, h, px)
        return w, h, px

    def alive(self):
        return self.proc.poll() is None

    def wait_exit(self, timeout):
        try:
            self.proc.wait(timeout)
            return True
        except subprocess.TimeoutExpired:
            return False

    def stop(self):
        if self.alive():
            self.proc.kill()
            self.proc.wait()
        shutil.rmtree(self.dir, ignore_errors=True)


def write_png(path, w, h, px):
    raw = b"".join(b"\0" + px[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def chunk(t, b):
        return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b) & 0xffffffff)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def pixel(shot, x, y):
    w, h, px = shot
    i = (y * w + x) * 3
    return tuple(px[i:i + 3])
