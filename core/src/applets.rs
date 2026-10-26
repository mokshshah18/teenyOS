// The applet table: every command the shell can run, by name. The applets
// themselves live in text.rs and files.rs (shared with the Linux edition) and
// in system-ish wrappers here.

use crate::rt::out;
use crate::sysb as sys;
use crate::util::*;

pub type Applet = fn(&[&[u8]]) -> i32;

pub fn find(name: &[u8]) -> Option<Applet> {
    Some(match name {
        b"echo" => crate::text::echo,
        b"cat" => crate::text::cat,
        b"wc" => crate::text::wc,
        b"head" => crate::text::head,
        b"tail" => crate::text::tail,
        b"grep" => crate::text::grep,
        b"sort" => crate::text::sort,
        b"uniq" => crate::text::uniq,
        b"tee" => crate::text::tee,
        b"seq" => crate::text::seq,
        b"basename" => crate::text::basename,
        b"dirname" => crate::text::dirname,
        b"hexdump" => crate::text::hexdump,
        b"more" => crate::text::more,
        b"true" => crate::text::true_,
        b"false" => crate::text::false_,
        b"test" | b"[" => crate::text::test,
        b"ls" => crate::files::ls,
        b"cp" => crate::files::cp,
        b"mv" => crate::files::mv,
        b"rm" => crate::files::rm,
        b"mkdir" => crate::files::mkdir,
        b"rmdir" => crate::files::rmdir,
        b"ln" => crate::files::ln,
        b"touch" => crate::files::touch,
        b"pwd" => crate::files::pwd,
        b"chmod" => crate::files::chmod,
        b"stat" => crate::files::stat,
        b"clear" => clear,
        b"uname" => uname,
        b"date" => date,
        b"free" => free,
        b"ps" => ps,
        b"env" => env,
        b"net" => crate::net::net,
        b"wget" => crate::net::wget,
        b"persist" => crate::persist::main,
        _ => return None,
    })
}

fn clear(_: &[&[u8]]) -> i32 {
    out(b"\x1b[H\x1b[2J");
    0
}

fn uname(args: &[&[u8]]) -> i32 {
    let all = args.get(1).map_or(false, |a| a.contains(&b'a'));
    if all {
        out(b"teenyOS core 1.0 x86_64 UEFI\n");
    } else {
        out(b"teenyOS\n");
    }
    0
}

fn date(_: &[&[u8]]) -> i32 {
    let t = sys::now();
    let (y, m, d) = civil((t / 86400) as i64);
    let s = t % 86400;
    out(&MONTHS[(m as usize - 1) * 3..m as usize * 3]);
    out(b" ");
    out(&two(d as u64));
    out(b" ");
    out(&two(s / 3600));
    out(b":");
    out(&two(s / 60 % 60));
    out(b":");
    out(&two(s % 60));
    out(b" UTC ");
    out_i64(y);
    out(b"\n");
    0
}

fn free(_: &[&[u8]]) -> i32 {
    // We manage one big firmware block; report its use from the allocator.
    let (used, total) = crate::rt::heap_stats();
    out(b"         total       used       free\nMem: ");
    for v in [total, used, total - used] {
        out_pad(&human(v as u64), 11);
    }
    out(b"\n");
    0
}

fn ps(_: &[&[u8]]) -> i32 {
    out(b"  threads running: ");
    out_num(crate::sched::thread_count() as u64);
    out(b"\n  (each terminal window is a shell thread; no processes -- this is\n   one program with green threads on the firmware.)\n");
    0
}

fn env(_: &[&[u8]]) -> i32 {
    for e in crate::rt::env() {
        out(&e);
        out(b"\n");
    }
    0
}
