// Networking through the firmware. teenyOS core ships no TCP/IP stack, no DNS
// client and no TLS: it borrows the firmware's, which every UEFI machine has
// (the probe found HTTP, DNS, DHCP and TLS service bindings). `wget` drives
// the EFI HTTP protocol, which resolves names, speaks HTTP, and — because the
// firmware links in TLS — fetches https URLs too, where the firmware trusts
// the server's certificate.

use crate::rt::out;
use crate::sysb as sys;
use crate::uefi::{self, Handle};
use crate::util::*;
use alloc::vec;
use alloc::vec::Vec;

/// `net`: report what the firmware offers.
pub fn net(_: &[&[u8]]) -> i32 {
    let nics = uefi::count_protocol(&uefi::SNP_GUID);
    out(b"Networking is provided by the firmware (teenyOS ships no stack).\n\n");
    out(b"  network adapters (SNP) : ");
    out_num(nics as u64);
    out(b"\n");
    let caps: [(&[u8], &uefi::Guid); 5] = [
        (b"DHCP (get an address)  ", &uefi::DHCP4_SB_GUID),
        (b"DNS (resolve names)    ", &uefi::DNS4_SB_GUID),
        (b"HTTP                   ", &uefi::HTTP_SB_GUID),
        (b"TLS (so HTTPS works)   ", &uefi::TLS_SB_GUID),
        (b"IP configuration       ", &uefi::IP4CONFIG2_GUID),
    ];
    for (name, g) in caps {
        out(b"  ");
        out(name);
        out(b": ");
        out(if uefi::count_protocol(g) > 0 { b"yes\n" } else { b"no\n" });
    }
    out(b"\nUse `wget http://host/path` or `wget https://host/path`.\n");
    if nics == 0 {
        out(b"\nNo adapter is up yet. On a cable network DHCP runs on its own;\n");
        out(b"otherwise plug one in, or a phone sharing over USB.\n");
    }
    0
}

fn to_utf16(s: &[u8]) -> Vec<u16> {
    let mut v: Vec<u16> = s.iter().map(|&b| b as u16).collect();
    v.push(0);
    v
}

/// Put every network adapter on DHCP, as HTTP-boot firmware does. Without
/// this the firmware's IPv4 policy stays Static and no address is leased.
fn bring_up_dhcp() {
    let (hs, n) = uefi::locate_handles(&uefi::IP4CONFIG2_GUID);
    if hs.is_null() {
        return;
    }
    for i in 0..n {
        let cfg = uefi::handle_protocol(unsafe { *hs.add(i) }, &uefi::IP4CONFIG2_GUID);
        if cfg.is_null() {
            continue;
        }
        unsafe {
            // SetData(This, Ip4Config2DataTypePolicy=1, 4, &Dhcp=1)
            let set: extern "efiapi" fn(*mut u8, u32, usize, *const u8) -> usize = core::mem::transmute(core::ptr::read_unaligned(cfg as *const usize));
            let policy = 1u32.to_le_bytes();
            set(cfg, 1, 4, policy.as_ptr());
        }
    }
    uefi::raw_free(hs as *mut u8);
}

/// One HTTP(S) GET through the firmware. Returns (status code, body).
fn http_get(url: &[u8], host: &[u8]) -> Result<(u16, Vec<u8>), &'static [u8]> {
    bring_up_dhcp();
    // Bring up a HTTP protocol child on the first HTTP-capable handle.
    let (hs, n) = uefi::locate_handles(&uefi::HTTP_SB_GUID);
    if hs.is_null() || n == 0 {
        return Err(b"the firmware exposes no HTTP support on any adapter");
    }
    let sb = uefi::handle_protocol(unsafe { *hs }, &uefi::HTTP_SB_GUID);
    uefi::raw_free(hs as *mut u8);
    if sb.is_null() {
        return Err(b"could not open the HTTP service binding");
    }
    let child = uefi::sb_create_child(sb);
    if child.is_null() {
        return Err(b"the firmware would not create an HTTP session");
    }
    let http = uefi::handle_protocol(child, &uefi::HTTP_GUID);
    if http.is_null() {
        uefi::sb_destroy_child(sb, child);
        return Err(b"no HTTP protocol on the session");
    }

    // Prefer DHCP (real networks). If no address is leased in time, retry
    // with a static address -- QEMU user networking, and many simple setups,
    // accept a guest that picks 10.0.2.15/24 with gateway 10.0.2.2.
    let r1 = unsafe { http_exchange(http, url, host, true) };
    if r1.is_ok() {
        uefi::sb_destroy_child(sb, child);
        return r1;
    }
    let child2 = uefi::sb_create_child(sb);
    let http2 = if child2.is_null() { core::ptr::null_mut() } else { uefi::handle_protocol(child2, &uefi::HTTP_GUID) };
    let result = if http2.is_null() { r1 } else { unsafe { http_exchange(http2, url, host, false) } };
    uefi::sb_destroy_child(sb, child);
    if !child2.is_null() {
        uefi::sb_destroy_child(sb, child2);
    }
    result
}

unsafe fn vtbl(p: *mut u8, i: usize) -> usize {
    core::ptr::read_unaligned(p.add(i * 8) as *const usize)
}

unsafe fn http_exchange(http: *mut u8, url: &[u8], host: &[u8], use_dhcp: bool) -> Result<(u16, Vec<u8>), &'static [u8]> {
    // EFI_HTTP_CONFIG_DATA: version(u32)+pad, timeout(u32), ipv6(bool)+pad,
    // *AccessPoint. The IPv4 node asks the firmware to use DHCP.
    // IPv4 node: UseDefaultAddress(bool) + LocalAddress[4] + Subnet[4] + port(u16)
    let mut v4node = [0u8; 16];
    if use_dhcp {
        v4node[0] = 1; // UseDefaultAddress = TRUE -> DHCP
    } else {
        v4node[0] = 0;
        v4node[1..5].copy_from_slice(&[10, 0, 2, 15]);     // LocalAddress
        v4node[5..9].copy_from_slice(&[255, 255, 255, 0]); // LocalSubnet
    }
    let mut config = [0u8; 32];
    config[0..4].copy_from_slice(&1u32.to_le_bytes()); // HttpVersion11
    config[4..8].copy_from_slice(&30000u32.to_le_bytes()); // 30s timeout
    config[16..24].copy_from_slice(&(v4node.as_ptr() as u64).to_le_bytes());
    let configure: extern "efiapi" fn(*mut u8, *const u8) -> usize = core::mem::transmute(vtbl(http, 1));
    let r = configure(http, config.as_ptr());
    if r != 0 {
        return Err(b"could not configure HTTP (is the network up? try `net`)");
    }

    let event = uefi::create_event();
    if event.is_null() {
        return Err(b"out of events");
    }

    // Build the request. EFI_HTTP_REQUEST_DATA { method(u32)+pad, *Url }.
    let url16 = to_utf16(url);
    let mut req = [0u8; 16];
    req[0..4].copy_from_slice(&0u32.to_le_bytes()); // GET
    req[8..16].copy_from_slice(&(url16.as_ptr() as u64).to_le_bytes());
    // One header: Host.
    let host_name = b"Host\0";
    let mut host_val = host.to_vec();
    host_val.push(0);
    let ua = b"User-Agent\0";
    let ua_val = b"teenyOS-core\0";
    let headers = [
        [host_name.as_ptr() as u64, host_val.as_ptr() as u64],
        [ua.as_ptr() as u64, ua_val.as_ptr() as u64],
    ];
    // EFI_HTTP_MESSAGE { *Data, HeaderCount, *Headers, BodyLength, *Body }.
    let mut msg = [0u8; 40];
    msg[0..8].copy_from_slice(&(req.as_ptr() as u64).to_le_bytes());
    msg[8..16].copy_from_slice(&2u64.to_le_bytes());
    msg[16..24].copy_from_slice(&(headers.as_ptr() as u64).to_le_bytes());
    // EFI_HTTP_TOKEN { Event, Status, *Message }.
    let mut token = [0u8; 24];
    token[0..8].copy_from_slice(&(event as u64).to_le_bytes());
    token[16..24].copy_from_slice(&(msg.as_ptr() as u64).to_le_bytes());

    let request: extern "efiapi" fn(*mut u8, *const u8) -> usize = core::mem::transmute(vtbl(http, 2));
    let poll: extern "efiapi" fn(*mut u8) -> usize = core::mem::transmute(vtbl(http, 5));
    // Configure starts DHCP asynchronously; Request fails with EFI_NO_MAPPING
    // until an address is assigned. Pump the stack and retry for a while.
    const NO_MAPPING: usize = 0x8000_0000_0000_0000 | 0x11;
    let mut rq = NO_MAPPING;
    // Real time, not the virtual ms counter: DHCP needs network round trips.
    let retries = if use_dhcp { 40 } else { 4 };
    for _ in 0..retries {
        rq = request(http, token.as_ptr());
        if rq != NO_MAPPING {
            break;
        }
        for _ in 0..20 {
            poll(http);
        }
        uefi::stall(200_000); // 200ms of real time per retry
        crate::sched::yield_now();
    }
    if rq != 0 {
        uefi::close_event(event);
        return Err(b"could not start the request (is the network up? see `net`)");
    }
    if !pump_until(http, poll, event, 750) {
        uefi::close_event(event);
        return Err(b"timed out sending the request");
    }
    uefi::close_event(event);

    // Response: first call carries the status + headers, then body chunks.
    let revent = uefi::create_event();
    let mut status_code = [0u8; 8]; // EFI_HTTP_RESPONSE_DATA { EFI_HTTP_STATUS_CODE }
    let mut rreq = [0u8; 8];
    rreq[0..8].copy_from_slice(&(status_code.as_mut_ptr() as u64).to_le_bytes());
    let mut body = vec![0u8; 64 * 1024];
    let mut out_body: Vec<u8> = Vec::new();
    let response: extern "efiapi" fn(*mut u8, *const u8) -> usize = core::mem::transmute(vtbl(http, 4));
    let mut code = 0u16;
    let mut first = true;
    let mut content_len: Option<usize> = None;
    loop {
        let mut rmsg = [0u8; 40];
        if first {
            rmsg[0..8].copy_from_slice(&(rreq.as_ptr() as u64).to_le_bytes());
        }
        rmsg[24..32].copy_from_slice(&(body.len() as u64).to_le_bytes());
        rmsg[32..40].copy_from_slice(&(body.as_mut_ptr() as u64).to_le_bytes());
        let mut rtoken = [0u8; 24];
        rtoken[0..8].copy_from_slice(&(revent as u64).to_le_bytes());
        rtoken[16..24].copy_from_slice(&(rmsg.as_ptr() as u64).to_le_bytes());
        let rr = response(http, rtoken.as_ptr());
        if rr != 0 {
            break;
        }
        if !pump_until(http, poll, revent, 750) {
            break;
        }
        if first {
            code = status_to_num(u32::from_le_bytes(status_code[0..4].try_into().unwrap()));
            // Read Content-Length from the firmware-allocated header array, so
            // we know exactly how many body bytes to keep.
            let hc = u64::from_le_bytes(rmsg[8..16].try_into().unwrap()) as usize;
            let hp = u64::from_le_bytes(rmsg[16..24].try_into().unwrap());
            if hp != 0 && hc > 0 {
                for k in 0..hc {
                    let h = hp as *const u8 as usize + k * 16;
                    let name = uefi::cstr8(core::ptr::read_unaligned(h as *const u64) as *const u8);
                    let value = uefi::cstr8(core::ptr::read_unaligned((h + 8) as *const u64) as *const u8);
                    if name.eq_ignore_ascii_case(b"content-length") {
                        content_len = parse_u64(trim(value)).map(|v| v as usize);
                    }
                }
                uefi::raw_free(hp as *mut u8);
            }
            first = false;
        }
        let mut got = u64::from_le_bytes(rmsg[24..32].try_into().unwrap()) as usize;
        if let Some(cl) = content_len {
            // Never keep more than the advertised length.
            got = got.min(cl.saturating_sub(out_body.len()));
        }
        if got > 0 {
            out_body.extend_from_slice(&body[..got]);
        }
        if let Some(cl) = content_len {
            if out_body.len() >= cl {
                break;
            }
        }
        let tstatus = usize::from_le_bytes(rtoken[8..16].try_into().unwrap());
        if got == 0 && tstatus != 0 {
            break; // connection closed, no more data
        }
        if content_len.is_none() && got == 0 {
            break;
        }
        if out_body.len() > 32 * 1024 * 1024 {
            break;
        }
    }
    uefi::close_event(revent);
    Ok((code, out_body))
}

/// Pump the firmware network stack until `event` signals or we time out.
/// `steps` of ~20ms real time each.
fn pump_until(http: *mut u8, poll: extern "efiapi" fn(*mut u8) -> usize, event: Handle, steps: u64) -> bool {
    for _ in 0..steps {
        if uefi::check_event(event) {
            return true;
        }
        poll(http);
        uefi::stall(20_000);
        // Yield so the desktop and other threads keep running during the fetch.
        crate::sched::yield_now();
    }
    uefi::check_event(event)
}

/// Map an EFI_HTTP_STATUS_CODE enum index to the numeric HTTP status.
fn status_to_num(idx: u32) -> u16 {
    const CODES: &[u16] = &[
        0, 100, 101, 200, 201, 202, 203, 204, 205, 206, 300, 301, 302, 303, 304, 305, 307, 400, 401,
        402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414, 415, 416, 417, 500, 501,
        502, 503, 504, 505, 308, 426,
    ];
    *CODES.get(idx as usize).unwrap_or(&0)
}

pub fn wget(args: &[&[u8]]) -> i32 {
    let (fl, ops) = crate::shell::opts(args, b"O");
    let Some(&url) = ops.first() else {
        msg(b"wget", b"usage: wget [-O FILE] http(s)://host/path");
        return 2;
    };
    let rest = if let Some(r) = url.strip_prefix(b"https://") {
        r
    } else if let Some(r) = url.strip_prefix(b"http://") {
        r
    } else {
        msg(b"wget", b"the url must start with http:// or https://");
        return 1;
    };
    let host_end = rest.iter().position(|&c| c == b'/' || c == b':').unwrap_or(rest.len());
    let host = &rest[..host_end];
    out(b"Fetching ");
    out(url);
    out(b" ...\n");
    crate::rt::flush();
    match http_get(url, host) {
        Ok((code, body)) => {
            if code != 0 && !(200..300).contains(&code) {
                out(b"server returned ");
                out_num(code as u64);
                out(b"\n");
                return 1;
            }
            let name = crate::shell::val(&fl, b'O').map(|v| v.to_vec()).unwrap_or_else(|| {
                let path = &rest[host_end..];
                let b = basename(path.split(|&c| c == b'?').next().unwrap_or(b"/"));
                if b.is_empty() || b == b"/" { b"index.html".to_vec() } else { b.to_vec() }
            });
            if name == b"-" {
                out(&body);
                return 0;
            }
            match write_file(&name, &body) {
                Ok(()) => {
                    out(b"saved ");
                    out(&name);
                    out(b" (");
                    out(&human(body.len() as u64));
                    out(b")\n");
                    0
                }
                Err(e) => {
                    perr(b"wget", &name, e);
                    1
                }
            }
        }
        Err(e) => {
            msg(b"wget", e);
            1
        }
    }
}
