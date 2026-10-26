// Networking: a DHCP client daemon, DNS lookups, and the net, ping, wget
// and share commands. IPv4 only; no Wi-Fi and no TLS (so no https).

use crate::rt::{flush, out};
use crate::sys;
use crate::util::*;
use alloc::vec;
use alloc::vec::Vec;

const SYS_SOCKET: usize = 41;
const SYS_CONNECT: usize = 42;
const SYS_ACCEPT: usize = 43;
const SYS_SENDTO: usize = 44;
const SYS_RECVFROM: usize = 45;
const SYS_BIND: usize = 49;
const SYS_LISTEN: usize = 50;
const SYS_SETSOCKOPT: usize = 54;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOCK_DGRAM: usize = 2;
const SOCK_RAW: usize = 3;
const SOL_SOCKET: usize = 1;

pub type Ip = [u8; 4];

fn sockaddr(ip: Ip, port: u16) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0] = AF_INET as u8;
    a[2..4].copy_from_slice(&port.to_be_bytes());
    a[4..8].copy_from_slice(&ip);
    a
}

fn socket(kind: usize, proto: usize) -> i32 {
    sys::sc(SYS_SOCKET, AF_INET, kind | sys::O_CLOEXEC, proto, 0, 0) as i32
}

fn setopt(fd: i32, level: usize, opt: usize, val: &[u8]) {
    sys::sc(SYS_SETSOCKOPT, fd as usize, level, opt, val.as_ptr() as usize, val.len());
}

fn sendto(fd: i32, data: &[u8], to: &[u8; 16]) -> isize {
    sys::sc6(SYS_SENDTO, fd as usize, data.as_ptr() as usize, data.len(), 0, to.as_ptr() as usize, 16)
}

fn recv(fd: i32, buf: &mut [u8]) -> isize {
    sys::sc6(SYS_RECVFROM, fd as usize, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0)
}

/// Wait up to `ms` for `fd` to become readable.
fn readable(fd: i32, ms: i32) -> bool {
    let mut p = [sys::PollFd { fd, events: sys::POLLIN, revents: 0 }];
    sys::poll(&mut p, ms) > 0
}

pub fn fmt_ip(ip: Ip) -> Vec<u8> {
    let mut v = Vec::new();
    for (i, b) in ip.iter().enumerate() {
        if i > 0 {
            v.push(b'.');
        }
        v.extend_from_slice(&num(*b as u64));
    }
    v
}

pub fn parse_ip(s: &[u8]) -> Option<Ip> {
    let parts: Vec<&[u8]> = s.split(|&c| c == b'.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut ip = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        let n = parse_u64(p)?;
        if n > 255 {
            return None;
        }
        ip[i] = n as u8;
    }
    Some(ip)
}

// ---------------------------------------------------------------- interfaces

const SIOCSIFFLAGS: usize = 0x8914;
const SIOCGIFFLAGS: usize = 0x8913;
const SIOCSIFADDR: usize = 0x8916;
const SIOCSIFNETMASK: usize = 0x891c;
const SIOCGIFHWADDR: usize = 0x8927;
const SIOCADDRT: usize = 0x890b;

fn ifreq(name: &[u8]) -> [u8; 40] {
    let mut r = [0u8; 40];
    let n = name.len().min(15);
    r[..n].copy_from_slice(&name[..n]);
    r
}

fn if_ioctl(req: usize, r: &mut [u8; 40]) -> isize {
    let fd = socket(SOCK_DGRAM, 0);
    let x = sys::ioctl(fd, req, r.as_mut_ptr() as usize);
    sys::close(fd);
    x
}

fn if_up(name: &[u8]) {
    let mut r = ifreq(name);
    if_ioctl(SIOCGIFFLAGS, &mut r);
    r[16] |= 1; // IFF_UP
    if_ioctl(SIOCSIFFLAGS, &mut r);
}

fn mac(name: &[u8]) -> [u8; 6] {
    let mut r = ifreq(name);
    if_ioctl(SIOCGIFHWADDR, &mut r);
    r[18..24].try_into().unwrap()
}

fn set_addr(name: &[u8], ip: Ip, mask: Ip) {
    let mut r = ifreq(name);
    r[16..32].copy_from_slice(&sockaddr(ip, 0));
    if_ioctl(SIOCSIFADDR, &mut r);
    let mut r = ifreq(name);
    r[16..32].copy_from_slice(&sockaddr(mask, 0));
    if_ioctl(SIOCSIFNETMASK, &mut r);
}

fn add_default_route(gw: Ip) {
    // struct rtentry: dst at 8, gateway at 24, genmask at 40, flags at 56.
    let mut rt = [0u8; 120];
    rt[8..24].copy_from_slice(&sockaddr([0; 4], 0));
    rt[24..40].copy_from_slice(&sockaddr(gw, 0));
    rt[40..56].copy_from_slice(&sockaddr([0; 4], 0));
    rt[56] = 1 | 2; // RTF_UP | RTF_GATEWAY
    let fd = socket(SOCK_DGRAM, 0);
    sys::ioctl(fd, SIOCADDRT, rt.as_ptr() as usize);
    sys::close(fd);
}

fn interfaces() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = list_dir(b"/sys/class/net").unwrap_or_default().into_iter().filter(|n| n != b"lo").collect();
    v.sort();
    v
}

fn carrier(name: &[u8]) -> bool {
    read_file(&[b"/sys/class/net/" as &[u8], name, b"/carrier"].concat()).map_or(false, |d| trim(&d) == b"1")
}

// ---------------------------------------------------------------- DHCP

struct Lease {
    ip: Ip,
    mask: Ip,
    router: Ip,
    dns: Ip,
    secs: u64,
}

fn dhcp_packet(kind: u8, xid: u32, hw: [u8; 6], want: Option<(Ip, Ip)>) -> Vec<u8> {
    let mut p = vec![0u8; 240];
    p[0] = 1; // request
    p[1] = 1; // ethernet
    p[2] = 6;
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[10] = 0x80; // please broadcast the reply: we have no address yet
    p[28..34].copy_from_slice(&hw);
    p[236..240].copy_from_slice(&[99, 130, 83, 99]);
    p.extend_from_slice(&[53, 1, kind]);
    if let Some((ip, server)) = want {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&ip);
        p.extend_from_slice(&[54, 4]);
        p.extend_from_slice(&server);
    }
    p.extend_from_slice(&[12, 5]);
    p.extend_from_slice(b"teeny");
    p.extend_from_slice(&[55, 4, 1, 3, 6, 51, 255]);
    p.resize(300, 0);
    p
}

/// (message type, offered ip, options) from a reply for our xid.
fn dhcp_parse(p: &[u8], xid: u32) -> Option<(u8, Ip, Lease, Ip)> {
    if p.len() < 240 || p[0] != 2 || p[4..8] != xid.to_be_bytes() || p[236..240] != [99, 130, 83, 99] {
        return None;
    }
    let yi: Ip = p[16..20].try_into().unwrap();
    let mut l = Lease { ip: yi, mask: [255, 255, 255, 0], router: [0; 4], dns: [0; 4], secs: 3600 };
    let (mut kind, mut server) = (0u8, [0u8; 4]);
    let mut i = 240;
    while i + 1 < p.len() && p[i] != 255 {
        if p[i] == 0 {
            i += 1;
            continue;
        }
        let (code, len) = (p[i], p[i + 1] as usize);
        let v = p.get(i + 2..i + 2 + len)?;
        let ip4 = || -> Ip { v.get(..4).map_or([0; 4], |s| s.try_into().unwrap()) };
        match code {
            53 => kind = v[0],
            1 => l.mask = ip4(),
            3 => l.router = ip4(),
            6 => l.dns = ip4(),
            51 => l.secs = u32::from_be_bytes(ip4()) as u64,
            54 => server = ip4(),
            _ => {}
        }
        i += 2 + len;
    }
    Some((kind, yi, l, server))
}

fn dhcp(name: &[u8]) -> Option<Lease> {
    let fd = socket(SOCK_DGRAM, 0);
    if fd < 0 {
        return None;
    }
    let one = 1i32.to_le_bytes();
    setopt(fd, SOL_SOCKET, 2, &one); // SO_REUSEADDR
    setopt(fd, SOL_SOCKET, 6, &one); // SO_BROADCAST
    let mut dev = name.to_vec();
    dev.push(0);
    setopt(fd, SOL_SOCKET, 25, &dev); // SO_BINDTODEVICE
    let me = sockaddr([0; 4], 68);
    sys::sc(SYS_BIND, fd as usize, me.as_ptr() as usize, 16, 0, 0);
    let to = sockaddr([255; 4], 67);
    let hw = mac(name);
    let xid = (sys::now_ms() as u32) ^ u32::from_le_bytes([hw[2], hw[3], hw[4], hw[5]]);
    let mut buf = [0u8; 1500];
    let mut result = None;
    'tries: for _ in 0..4 {
        sendto(fd, &dhcp_packet(1, xid, hw, None), &to);
        let t = sys::now_ms();
        while sys::now_ms() - t < 3000 {
            if !readable(fd, 500) {
                continue;
            }
            let n = recv(fd, &mut buf);
            let Some((2, offer, _, server)) = (if n > 0 { dhcp_parse(&buf[..n as usize], xid) } else { None }) else { continue };
            sendto(fd, &dhcp_packet(3, xid, hw, Some((offer, server))), &to);
            let t2 = sys::now_ms();
            while sys::now_ms() - t2 < 3000 {
                if !readable(fd, 500) {
                    continue;
                }
                let n = recv(fd, &mut buf);
                match if n > 0 { dhcp_parse(&buf[..n as usize], xid) } else { None } {
                    Some((5, _, lease, _)) => {
                        result = Some(lease);
                        break 'tries;
                    }
                    Some((6, ..)) => continue 'tries,
                    _ => {}
                }
            }
        }
    }
    sys::close(fd);
    result
}

/// `net daemon`: started by init. Brings interfaces up as they appear
/// (including phones plugged in for USB tethering) and keeps leases fresh.
fn daemon() -> i32 {
    if_up(b"lo");
    let mut leases: Vec<(Vec<u8>, u64)> = Vec::new(); // (interface, renew at)
    loop {
        let ifs = interfaces();
        leases.retain(|(n, _)| ifs.contains(n) && carrier(n));
        for name in &ifs {
            if_up(name);
            let due = leases.iter().find(|(n, _)| n == name).map(|(_, t)| *t);
            if !carrier(name) || due.map_or(false, |t| sys::now() < t) {
                continue;
            }
            if let Some(l) = dhcp(name) {
                set_addr(name, l.ip, l.mask);
                if l.router != [0; 4] {
                    add_default_route(l.router);
                }
                let status = [&name[..], b" ", &fmt_ip(l.ip), b" gateway ", &fmt_ip(l.router), b" dns ", &fmt_ip(l.dns), b"\n"].concat();
                let _ = write_file(b"/run/net", &status);
                let _ = write_file(b"/run/resolv", &fmt_ip(if l.dns == [0; 4] { l.router } else { l.dns }));
                leases.retain(|(n, _)| n != name);
                leases.push((name.clone(), sys::now() + (l.secs / 2).max(60)));
            }
        }
        if leases.is_empty() {
            sys::unlink(b"/run/net");
        }
        sys::sleep_ms(3000);
    }
}

// ---------------------------------------------------------------- DNS

pub fn resolve(host: &[u8]) -> Option<Ip> {
    if let Some(ip) = parse_ip(host) {
        return Some(ip);
    }
    let server = read_file(b"/run/resolv").ok().and_then(|d| parse_ip(trim(&d)))?;
    let fd = socket(SOCK_DGRAM, 0);
    let id = (sys::now_ms() & 0xffff) as u16;
    let mut q = Vec::new();
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]); // recursion, 1 question
    for label in host.split(|&c| c == b'.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label);
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // type A, class IN
    let to = sockaddr(server, 53);
    let mut buf = [0u8; 1500];
    for _ in 0..3 {
        sendto(fd, &q, &to);
        if !readable(fd, 2000) {
            continue;
        }
        let n = recv(fd, &mut buf);
        if n < 12 || buf[0..2] != id.to_be_bytes() {
            continue;
        }
        let r = &buf[..n as usize];
        let answers = u16::from_be_bytes([r[6], r[7]]);
        let mut i = q.len(); // skip the echoed question
        for _ in 0..answers {
            // Name: labels or a compression pointer.
            while i < r.len() && r[i] != 0 && r[i] & 0xc0 != 0xc0 {
                i += r[i] as usize + 1;
            }
            i += if i < r.len() && r[i] & 0xc0 == 0xc0 { 2 } else { 1 };
            if i + 10 > r.len() {
                break;
            }
            let kind = u16::from_be_bytes([r[i], r[i + 1]]);
            let len = u16::from_be_bytes([r[i + 8], r[i + 9]]) as usize;
            i += 10;
            if kind == 1 && len == 4 && i + 4 <= r.len() {
                sys::close(fd);
                return Some(r[i..i + 4].try_into().unwrap());
            }
            i += len;
        }
        break;
    }
    sys::close(fd);
    None
}

// ---------------------------------------------------------------- commands

fn status() -> i32 {
    let ifs = interfaces();
    if ifs.is_empty() {
        out(b"No network adapter found. Plug in a cable, a USB Ethernet adapter,\nor a phone with USB tethering turned on.\n");
        return 1;
    }
    for n in &ifs {
        out_padr(n, 10);
        out(if carrier(n) { b"cable/link up" } else { b"no link" });
        let m = mac(n);
        out(b"   ");
        let hex = b"0123456789abcdef";
        for (i, b) in m.iter().enumerate() {
            if i > 0 {
                out(b":");
            }
            out(&[hex[(b >> 4) as usize], hex[(b & 15) as usize]]);
        }
        out(b"\n");
    }
    match read_file(b"/run/net") {
        Ok(s) if !s.is_empty() => {
            out(b"\nOnline: ");
            out(&s);
        }
        _ => out(b"\nOffline (waiting for a network that hands out addresses).\n"),
    }
    0
}

fn checksum(d: &[u8]) -> u16 {
    let mut s: u32 = 0;
    for c in d.chunks(2) {
        s += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while s > 0xffff {
        s = (s & 0xffff) + (s >> 16);
    }
    !(s as u16)
}

pub fn ping(args: &[&[u8]]) -> i32 {
    let (fl, ops) = crate::text::opts(args, b"c");
    let count = crate::text::val(&fl, b'c').and_then(parse_u64).unwrap_or(4);
    let Some(host) = ops.first() else {
        msg(b"ping", b"usage: ping [-c COUNT] HOST");
        return 2;
    };
    let Some(ip) = resolve(host) else {
        msg(b"ping", b"can't find that host (are you online? see `net`)");
        return 2;
    };
    let fd = socket(SOCK_RAW, 1); // IPPROTO_ICMP
    if fd < 0 {
        perr(b"ping", b"socket", fd as isize);
        return 2;
    }
    out(b"PING ");
    out(host);
    out(b" (");
    out(&fmt_ip(ip));
    out(b")\n");
    flush();
    let to = sockaddr(ip, 0);
    let id = (sys::getpid() & 0xffff) as u16;
    let mut got = 0;
    for seq in 1..=count as u16 {
        let mut pkt = vec![8u8, 0, 0, 0];
        pkt.extend_from_slice(&id.to_be_bytes());
        pkt.extend_from_slice(&seq.to_be_bytes());
        pkt.extend_from_slice(b"teenyOS ping payload....");
        let c = checksum(&pkt);
        pkt[2..4].copy_from_slice(&c.to_be_bytes());
        let t = sys::now_ms();
        sendto(fd, &pkt, &to);
        let mut buf = [0u8; 1500];
        let mut ok = false;
        while sys::now_ms() - t < 2000 {
            if !readable(fd, 200) {
                continue;
            }
            let n = recv(fd, &mut buf);
            if n < 28 {
                continue;
            }
            let ihl = (buf[0] & 15) as usize * 4;
            let icmp = &buf[ihl..n as usize];
            if icmp.len() >= 8 && icmp[0] == 0 && icmp[4..6] == id.to_be_bytes() && icmp[6..8] == seq.to_be_bytes() {
                out(b"reply from ");
                out(&fmt_ip(ip));
                out(b": seq=");
                out_num(seq as u64);
                out(b" time=");
                out_num(sys::now_ms() - t);
                out(b" ms\n");
                flush();
                ok = true;
                got += 1;
                break;
            }
        }
        if !ok {
            out(b"no reply, seq=");
            out_num(seq as u64);
            out(b"\n");
            flush();
        }
        if seq as u64 != count {
            sys::sleep_ms(1000u64.saturating_sub(sys::now_ms() - t));
        }
    }
    sys::close(fd);
    if got > 0 {
        0
    } else {
        1
    }
}

fn connect(ip: Ip, port: u16) -> i32 {
    let fd = socket(SOCK_STREAM, 0);
    let a = sockaddr(ip, port);
    if sys::sc(SYS_CONNECT, fd as usize, a.as_ptr() as usize, 16, 0, 0) < 0 {
        sys::close(fd);
        return -1;
    }
    fd
}

pub fn wget(args: &[&[u8]]) -> i32 {
    let (fl, ops) = crate::text::opts(args, b"O");
    let Some(&url0) = ops.first() else {
        msg(b"wget", b"usage: wget [-O FILE] http://host/path");
        return 2;
    };
    let mut url = url0.to_vec();
    for _ in 0..5 {
        if url.starts_with(b"https://") {
            msg(b"wget", b"https is not supported (teenyOS has no TLS); try http://");
            return 1;
        }
        let rest = url.strip_prefix(b"http://").unwrap_or(&url).to_vec();
        let slash = rest.iter().position(|&c| c == b'/').unwrap_or(rest.len());
        let (hostport, path) = (&rest[..slash], if slash < rest.len() { &rest[slash..] } else { &b"/"[..] });
        let (host, port) = match hostport.iter().position(|&c| c == b':') {
            Some(i) => (&hostport[..i], parse_u64(&hostport[i + 1..]).unwrap_or(80) as u16),
            None => (hostport, 80),
        };
        let Some(ip) = resolve(host) else {
            msg(b"wget", b"can't find that host (are you online? see `net`)");
            return 1;
        };
        let fd = connect(ip, port);
        if fd < 0 {
            msg(b"wget", b"connection refused");
            return 1;
        }
        let req = [b"GET " as &[u8], path, b" HTTP/1.0\r\nHost: ", host, b"\r\nUser-Agent: teenyOS-wget\r\nConnection: close\r\n\r\n"].concat();
        sys::write_all(fd, &req);
        let resp = read_fd(fd);
        sys::close(fd);
        let Some(hend) = find(&resp, b"\r\n\r\n") else {
            msg(b"wget", b"bad response");
            return 1;
        };
        let head = &resp[..hend];
        let code = head.get(9..12).and_then(parse_u64).unwrap_or(0);
        if (300..400).contains(&code) {
            if let Some(l) = lines(head).iter().find(|l| l.len() > 9 && l[..9].eq_ignore_ascii_case(b"location:")) {
                url = trim(&l[9..]).to_vec();
                out(b"redirected to ");
                out(&url);
                out(b"\n");
                continue;
            }
        }
        if code != 200 {
            out(lines(head).first().copied().unwrap_or(b""));
            out(b"\n");
            return 1;
        }
        let body = &resp[hend + 4..];
        let name = crate::text::val(&fl, b'O').map(|v| v.to_vec()).unwrap_or_else(|| {
            let b = basename(path.split(|&c| c == b'?').next().unwrap_or(b"/"));
            if b.is_empty() || b == b"/" { b"index.html".to_vec() } else { b.to_vec() }
        });
        if name == b"-" {
            out(body);
            return 0;
        }
        if let Err(e) = write_file(&name, body) {
            perr(b"wget", &name, e);
            return 1;
        }
        out(b"saved ");
        out(&name);
        out(b" (");
        out(&human(body.len() as u64));
        out(b")\n");
        return 0;
    }
    msg(b"wget", b"too many redirects");
    1
}

/// `share [DIR] [PORT]`: serve DIR over HTTP so other devices on the network
/// can download files from it, or upload into it from a browser.
pub fn share(args: &[&[u8]]) -> i32 {
    let dir = args.get(1).copied().unwrap_or(b"/root").to_vec();
    let port = args.get(2).and_then(|p| parse_u64(p)).unwrap_or(8080) as u16;
    let fd = socket(SOCK_STREAM, 0);
    setopt(fd, SOL_SOCKET, 2, &1i32.to_le_bytes());
    let a = sockaddr([0; 4], port);
    if sys::sc(SYS_BIND, fd as usize, a.as_ptr() as usize, 16, 0, 0) < 0 || sys::sc(SYS_LISTEN, fd as usize, 8, 0, 0, 0) < 0 {
        msg(b"share", b"can't listen on that port");
        return 1;
    }
    let me = read_file(b"/run/net").unwrap_or_default();
    let ip = me.split(|&c| c == b' ').nth(1).unwrap_or(b"<this machine>").to_vec();
    out(b"Sharing ");
    out(&dir);
    out(b". Open http://");
    out(&ip);
    out(b":");
    out_num(port as u64);
    out(b"/ in a browser on another device. Ctrl-C stops.\n");
    flush();
    loop {
        let c = sys::sc(SYS_ACCEPT, fd as usize, 0, 0, 0, 0) as i32;
        if c < 0 {
            continue;
        }
        serve_one(c, &dir);
        sys::close(c);
    }
}

fn serve_one(c: i32, root: &[u8]) {
    // Read the request head, then the body if there is one (uploads).
    let mut req = Vec::new();
    let mut buf = [0u8; 8192];
    let mut want = usize::MAX;
    while req.len() < want && req.len() < 64 << 20 {
        if !readable(c, 10_000) {
            return;
        }
        let n = sys::read(c, &mut buf);
        if n <= 0 {
            break;
        }
        req.extend_from_slice(&buf[..n as usize]);
        if want == usize::MAX {
            if let Some(h) = find(&req, b"\r\n\r\n") {
                let len = lines(&req[..h])
                    .iter()
                    .find(|l| l.len() > 15 && l[..15].eq_ignore_ascii_case(b"content-length:"))
                    .and_then(|l| parse_u64(trim(&l[15..])))
                    .unwrap_or(0);
                want = h + 4 + len as usize;
            }
        }
    }
    let Some(h) = find(&req, b"\r\n\r\n") else { return };
    let first = lines(&req[..h]).first().copied().unwrap_or(b"");
    let parts: Vec<&[u8]> = first.split(|&c| c == b' ').collect();
    let (method, raw) = (parts.first().copied().unwrap_or(b""), parts.get(1).copied().unwrap_or(b"/"));
    // Decode %XX and refuse paths that climb out of the shared folder.
    let mut path = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            path.push(hex(raw[i + 1]) << 4 | hex(raw[i + 2]));
            i += 3;
        } else {
            path.push(raw[i]);
            i += 1;
        }
    }
    if path.split(|&c| c == b'/').any(|s| s == b"..") {
        sys::write_all(c, b"HTTP/1.0 403 Forbidden\r\n\r\n");
        return;
    }
    let local = [root, &path[..]].concat();
    if method == b"POST" {
        let body = &req[h + 4..];
        let saved = save_upload(&req[..h], body, &local);
        let page = [b"<p>" as &[u8], if saved { b"Uploaded." } else { b"Upload failed." }, b" <a href=\"\">Back</a></p>"].concat();
        reply(c, b"200 OK", b"text/html", &page);
        return;
    }
    match sys::stat(&local) {
        Some(st) if st.is_dir() => {
            let mut names = list_dir(&local).unwrap_or_default();
            names.sort();
            let mut page = b"<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>teenyOS share</title><body style='font:16px sans-serif;max-width:40em;margin:2em auto'><h2>teenyOS: ".to_vec();
            page.extend_from_slice(&path);
            page.extend_from_slice(b"</h2><form method=post enctype=multipart/form-data><input type=file name=f> <button>Upload here</button></form><ul>");
            for n in names {
                let is_dir = sys::stat(&join(&local, &n)).map_or(false, |s| s.is_dir());
                page.extend_from_slice(b"<li><a href=\"");
                page.extend_from_slice(&n);
                if is_dir {
                    page.push(b'/');
                }
                page.extend_from_slice(b"\">");
                page.extend_from_slice(&n);
                if is_dir {
                    page.push(b'/');
                }
                page.extend_from_slice(b"</a>");
            }
            page.extend_from_slice(b"</ul>");
            reply(c, b"200 OK", b"text/html", &page);
        }
        Some(_) => match read_file(&local) {
            Ok(d) => reply(c, b"200 OK", b"application/octet-stream", &d),
            Err(_) => reply(c, b"403 Forbidden", b"text/plain", b"can't read that file"),
        },
        None => reply(c, b"404 Not Found", b"text/plain", b"not found"),
    }
}

fn reply(c: i32, status: &[u8], kind: &[u8], body: &[u8]) {
    let head = [b"HTTP/1.0 " as &[u8], status, b"\r\nContent-Type: ", kind, b"\r\nContent-Length: ", &num(body.len() as u64), b"\r\nConnection: close\r\n\r\n"].concat();
    sys::write_all(c, &head);
    sys::write_all(c, body);
}

/// Save the file part of a multipart/form-data upload into `dir`.
fn save_upload(head: &[u8], body: &[u8], dir: &[u8]) -> bool {
    let Some(b) = find(head, b"boundary=") else { return false };
    let bnd = head[b + 9..].split(|&c| c == b'\r' || c == b';').next().unwrap_or(b"");
    let delim = [b"--" as &[u8], bnd].concat();
    let Some(start) = find(body, &delim) else { return false };
    let part = &body[start + delim.len()..];
    let Some(ph) = find(part, b"\r\n\r\n") else { return false };
    let Some(fname_at) = find(&part[..ph], b"filename=\"") else { return false };
    let fname = &part[fname_at + 10..];
    let fname = basename(&fname[..fname.iter().position(|&c| c == b'"').unwrap_or(0)]);
    if fname.is_empty() {
        return false;
    }
    let data = &part[ph + 4..];
    let end = find(data, &[b"\r\n" as &[u8], &delim].concat()).unwrap_or(data.len());
    write_file(&join(dir, fname), &data[..end]).is_ok()
}

pub fn main(args: &[&[u8]]) -> i32 {
    match args.get(1).copied() {
        Some(b"daemon") => daemon(),
        None | Some(b"status") => status(),
        _ => {
            msg(b"net", b"usage: net [status]");
            1
        }
    }
}
