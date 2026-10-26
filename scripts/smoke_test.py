#!/usr/bin/env python3
"""Boot teenyOS in QEMU on a serial console, run commands, check the output.

usage: smoke_test.py IMAGE [MEM]
"""
import os
import select
import subprocess
import sys
import time

image = sys.argv[1]
mem = sys.argv[2] if len(sys.argv) > 2 else "8M"
PROMPT = b"# "

qemu = subprocess.Popen(
    ["qemu-system-i386", "-m", mem, "-kernel", image, "-nographic",
     "-no-reboot", "-append", "console=ttyS0 quiet"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
log = b""


def read_until(token, timeout):
    global log
    end = time.time() + timeout
    start = len(log)
    while time.time() < end:
        r, _, _ = select.select([qemu.stdout], [], [], 0.2)
        if r:
            chunk = os.read(qemu.stdout.fileno(), 4096)
            if not chunk:
                break
            log += chunk
            if token in log[start:]:
                return log[start:]
    raise TimeoutError(f"did not see {token!r}")


# The marker is typed with quotes, so only the command's output contains
# it verbatim, not the echo of what we typed.
MARK = b"@@DONE@@"


def run(cmd):
    qemu.stdin.write(cmd.encode() + b"\r")
    qemu.stdin.write(b'echo @@"DONE"@@\r')
    qemu.stdin.flush()
    out = read_until(MARK, 15)
    out = out[out.find(b"\n") + 1:out.rfind(MARK)]
    return out.decode(errors="replace")


# (command, text that must appear in its output)
CHECKS = [
    ("uname -sr", "Linux 6.6"),
    ("echo $HOME $((6*7))", "/root 42"),
    ("pwd", "/root"),
    ("ls /bin | wc -l", ""),
    ("for i in 1 2 3; do echo n$i; done | grep -c n", "3"),
    ("mkdir -p /tmp/a/b && echo hi > /tmp/a/b/f && cat /tmp/a/b/f", "hi"),
    ("cp -r /tmp/a /tmp/c && ls /tmp/c/b", "f"),
    ("mv /tmp/c/b/f /root/g && cat /root/g", "hi"),
    ("rm -r /tmp/a /tmp/c && ls /tmp | wc -l", "0"),
    ("x=0; while [ $x -lt 5 ]; do x=$((x+1)); done; echo x=$x", "x=5"),
    ("if test -d /proc; then echo yes; else echo no; fi", "yes"),
    ("seq 10 | sort -rn | head -n 2 | tail -n 1", "9"),
    ("echo $(echo nested) ok", "nested ok"),
    ("ps | grep -c sh", ""),
    ("cat /var/log/boot", "teenyOS: booted"),
    ("free", "Mem:"),
    ("mount | grep proc", "proc /proc proc"),
    ("hostname", "teeny"),
    ("date", "UTC 20"),
    ("a=$(date); sleep 2; b=$(date); test \"$a\" != \"$b\" && echo slept", "slept"),
    ("touch /tmp/new && ls /tmp", "new"),
    ("sleep 1 & wait; echo bg-done", "bg-done"),
    ("nosuchcmd", "not found"),
    ("printf_missing 2>/dev/null; echo $?", "127"),
    ("echo abc > /tmp/t; echo def >> /tmp/t; wc -l < /tmp/t", "2"),
    ("hexdump /tmp/t | head -n 1", "61 62 63 0a"),
    ("dmesg | grep -c 'Linux version'", "1"),
    ("ls /bin | grep -E '^sh$|^ls$' | wc -l", "2"),
]

failed = 0
try:
    read_until(PROMPT, 30)
    for cmd, want in CHECKS:
        body = run(cmd)
        ok = want in body
        failed += not ok
        print(("ok   " if ok else "FAIL ") + cmd)
        if not ok:
            print("     wanted:", repr(want))
            print("     got:   ", repr(body[-300:]))
    # Interactive pieces: the editor and tab completion.
    qemu.stdin.write(b"edit /tmp/e.txt\r")
    qemu.stdin.flush()
    time.sleep(1.5)
    # Type at a human pace: the emulated UART only buffers 16 bytes.
    for keys in [bytes([c]) for c in b"first line\rsecond"] + [b"\x1b[A", b"\x1b[F", b"!", b"\x13", b"\x11"]:
        qemu.stdin.write(keys)
        qemu.stdin.flush()
        time.sleep(0.1)
    time.sleep(0.5)
    # Ctrl-C must stop a foreground command and a shell loop around it.
    # Sync on a prompt first: a Ctrl-C typed ahead while the shell is still
    # reading a line is just an input byte, as in any shell.
    run("true")
    qemu.stdin.write(b"sleep 30 & while true; do sleep 5; done\r")
    qemu.stdin.flush()
    time.sleep(1)
    qemu.stdin.write(b"\x03")
    qemu.stdin.flush()
    time.sleep(0.5)
    for cmd, want in [("echo after-ctrl-c", "after-ctrl-c"),
                      ("ps | grep -c 'sleep 30'", "1"),
                      ("cat /tmp/e.txt", "first line!\r\nsecond"),
                      ("ech\t tab-ok", "tab-ok")]:
        body = run(cmd)
        ok = want in body
        failed += not ok
        print(("ok   " if ok else "FAIL ") + repr(cmd))
        if not ok:
            print("     got:   ", repr(body[-300:]))
    print(run("free"))
    qemu.stdin.write(b"poweroff\r")
    qemu.stdin.flush()
    time.sleep(2)
except TimeoutError as e:
    failed += 1
    print("FAIL", e)
    print(log.decode(errors="replace")[-2000:])
finally:
    qemu.kill()

total = len(CHECKS) + 4
print(f"{total}/{total} checks passed" if failed == 0 else f"{failed} failure(s)")
sys.exit(1 if failed else 0)
