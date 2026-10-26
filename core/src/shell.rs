// A minimal shell, used to validate the runtime end to end. It is replaced by
// the full ported shell once the plumbing is proven.

use crate::rt::out;
use crate::sysb as sys;
use crate::util::*;
use alloc::vec::Vec;

fn prompt() {
    let mut buf = [0u8; 512];
    let n = sys::getcwd(&mut buf);
    out(b"\x1b[1;32mroot@teeny\x1b[0m:\x1b[1;34m");
    if n > 1 {
        out(&buf[..n as usize - 1]);
    }
    out(b"\x1b[0m# ");
}

fn readline() -> Option<Vec<u8>> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let mut c = [0u8; 1];
        let r = sys::read(0, &mut c);
        if r <= 0 {
            return if line.is_empty() { None } else { Some(line) };
        }
        match c[0] {
            b'\r' | b'\n' => {
                out(b"\r\n");
                return Some(line);
            }
            8 | 127 => {
                if line.pop().is_some() {
                    out(b"\x08 \x08");
                }
            }
            3 => {
                out(b"^C\r\n");
                line.clear();
                return Some(line);
            }
            ch if ch >= 32 => {
                line.push(ch);
                out(&[ch]);
            }
            _ => {}
        }
    }
}

fn run(line: &[u8]) {
    let words: Vec<&[u8]> = line.split(|&c| c == b' ').filter(|w| !w.is_empty()).collect();
    let Some(&cmd) = words.first() else { return };
    match cmd {
        b"echo" => {
            for (i, w) in words[1..].iter().enumerate() {
                if i > 0 {
                    out(b" ");
                }
                out(w);
            }
            out(b"\n");
        }
        b"pwd" => {
            let mut buf = [0u8; 512];
            let n = sys::getcwd(&mut buf);
            out(&buf[..n as usize - 1]);
            out(b"\n");
        }
        b"cd" => {
            let dir = words.get(1).copied().unwrap_or(b"/root");
            if sys::chdir(dir) < 0 {
                out(b"cd: no such directory\n");
            }
        }
        b"ls" => {
            let dir = words.get(1).copied().unwrap_or(b".");
            match sys::list_dir(dir) {
                Ok(mut v) => {
                    v.sort();
                    for n in v {
                        out(&n);
                        out(b"\n");
                    }
                }
                Err(_) => out(b"ls: cannot read directory\n"),
            }
        }
        b"cat" => {
            for f in &words[1..] {
                match read_file(f) {
                    Ok(d) => out(&d),
                    Err(_) => out(b"cat: cannot open\n"),
                }
            }
        }
        b"mkdir" => {
            for d in &words[1..] {
                sys::mkdir(d, 0o755);
            }
        }
        b"write" => {
            // write FILE text...: quick helper to prove the filesystem works
            if words.len() >= 2 {
                let mut data = words[2..].join(&b' ');
                data.push(b'\n');
                let _ = write_file(words[1], &data);
            }
        }
        b"clear" => out(b"\x1b[H\x1b[2J"),
        b"uname" => out(b"teenyOS core x86_64 (UEFI)\n"),
        b"help" => out(b"builtins: echo pwd cd ls cat mkdir write clear uname help\n"),
        _ => {
            out(cmd);
            out(b": not found\n");
        }
    }
}

pub fn main() {
    out(b"\r\n  teenyOS core -- running on the firmware, no kernel.\r\n");
    out(b"  Try: ls /, echo hi > nowhere (soon), help\r\n\r\n");
    loop {
        prompt();
        match readline() {
            Some(line) if !trim(&line).is_empty() => run(trim(&line)),
            Some(_) => {}
            None => sys::sleep_ms(50),
        }
    }
}
