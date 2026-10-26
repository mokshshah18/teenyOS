// teeny: one static binary that provides init, a shell and the base
// utilities, picked by the name it was invoked under (like BusyBox).

#![no_std]
#![no_main]

extern crate alloc;

mod desktop;
mod disk;
mod edit;
mod fb;
mod files;
mod init;
mod install;
mod net;
mod persist;
mod rt;
mod sh;
mod sys;
mod system;
mod term;
mod text;
mod util;

pub type Applet = fn(&[&[u8]]) -> i32;

pub const APPLETS: &[(&[u8], Applet)] = &[
    (b"[", text::test),
    (b"basename", text::basename),
    (b"cat", text::cat),
    (b"chmod", files::chmod),
    (b"clear", system::clear),
    (b"cp", files::cp),
    (b"date", system::date),
    (b"desktop", desktop::main),
    (b"df", system::df),
    (b"dirname", text::dirname),
    (b"dmesg", system::dmesg),
    (b"echo", text::echo),
    (b"edit", edit::main),
    (b"env", system::env),
    (b"false", text::false_),
    (b"free", system::free),
    (b"grep", text::grep),
    (b"halt", system::halt),
    (b"head", text::head),
    (b"hexdump", text::hexdump),
    (b"hostname", system::hostname),
    (b"init", init::main),
    (b"kill", system::kill),
    (b"ln", files::ln),
    (b"ls", files::ls),
    (b"mkdir", files::mkdir),
    (b"more", text::more),
    (b"mount", system::mount),
    (b"mv", files::mv),
    (b"poweroff", system::poweroff),
    (b"net", net::main),
    (b"persist", persist::main),
    (b"ping", net::ping),
    (b"ps", system::ps),
    (b"pwd", files::pwd),
    (b"reboot", system::reboot),
    (b"rm", files::rm),
    (b"rmdir", files::rmdir),
    (b"seq", text::seq),
    (b"share", net::share),
    (b"sh", sh::main),
    (b"sleep", system::sleep),
    (b"sort", text::sort),
    (b"stat", files::stat),
    (b"sync", system::sync),
    (b"tail", text::tail),
    (b"teeny-install", install::main),
    (b"tee", text::tee),
    (b"test", text::test),
    (b"touch", files::touch),
    (b"true", text::true_),
    (b"umount", system::umount),
    (b"uname", system::uname),
    (b"uniq", text::uniq),
    (b"uptime", system::uptime),
    (b"wc", text::wc),
    (b"wget", net::wget),
    (b"which", system::which),
    (b"whoami", system::whoami),
];

pub fn find_applet(name: &[u8]) -> Option<Applet> {
    APPLETS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

fn main() -> i32 {
    let args = rt::args();
    let argv0 = args.first().copied().unwrap_or(b"teeny");
    let mut name = util::basename(argv0);
    // Login shells are started as "-sh".
    if name.first() == Some(&b'-') {
        name = &name[1..];
    }
    if let Some(f) = find_applet(name) {
        return f(&args);
    }
    // "teeny ls -l" works too.
    if args.len() > 1 {
        if let Some(f) = find_applet(args[1]) {
            return f(&args[1..]);
        }
    }
    rt::out(b"teeny: multi-call binary. Applets:\n");
    for (n, _) in APPLETS {
        rt::out(b" ");
        rt::out(n);
    }
    rt::out(b"\n");
    if args.len() > 1 {
        1
    } else {
        0
    }
}
