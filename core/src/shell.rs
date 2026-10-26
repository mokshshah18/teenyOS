// The shell. Not the full POSIX grammar, but genuinely useful: quoting,
// `$VAR`/`${VAR}`/`$?` and `$(...)` substitution, filename globbing, `;` `&&`
// `||` sequencing, `|` pipelines, and `>` `>>` `<` `2>` redirection.
//
// Pipelines are buffered rather than streamed: each stage runs to completion
// writing into an unbounded in-RAM pipe, then the next stage drains it. There
// are no kernel processes to run stages concurrently -- only cooperative green
// threads reaped the moment they exit -- so buffering is what keeps exit codes
// and output deterministic. For every finite command we ship the result is
// identical to a streamed pipe.

use crate::rt::out;
use crate::sysb as sys;
use crate::util::*;
use crate::{applets, sched};
use alloc::vec::Vec;

/// Single-letter option parser, shared with the applets.
pub fn opts<'a>(args: &[&'a [u8]], with_value: &[u8]) -> (Vec<(u8, &'a [u8])>, Vec<&'a [u8]>) {
    let mut flags = Vec::new();
    let mut ops = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = args[i];
        if a.len() > 1 && a[0] == b'-' && a[1] != b'-' && !a[1].is_ascii_digit() {
            let mut j = 1;
            while j < a.len() {
                let f = a[j];
                if with_value.contains(&f) {
                    let v: &[u8] = if j + 1 < a.len() {
                        &a[j + 1..]
                    } else {
                        i += 1;
                        args.get(i).copied().unwrap_or(b"")
                    };
                    flags.push((f, v));
                    break;
                }
                flags.push((f, b"" as &[u8]));
                j += 1;
            }
        } else {
            ops.push(a);
        }
        i += 1;
    }
    (flags, ops)
}

pub fn val<'a>(flags: &[(u8, &'a [u8])], f: u8) -> Option<&'a [u8]> {
    flags.iter().rev().find(|&&(c, _)| c == f).map(|&(_, v)| v)
}

// ---------------------------------------------------------------- line editor

fn build_prompt(last: i32) -> Vec<u8> {
    let mut p = Vec::new();
    if last != 0 {
        p.extend_from_slice(b"\x1b[31m");
        p.extend_from_slice(&num(last as u64));
        p.extend_from_slice(b" \x1b[0m");
    }
    p.extend_from_slice(b"\x1b[1;32mroot@teeny\x1b[0m:\x1b[1;34m");
    let mut buf = [0u8; 512];
    let n = sys::getcwd(&mut buf);
    if n > 1 {
        p.extend_from_slice(&buf[..n as usize - 1]);
    }
    p.extend_from_slice(b"\x1b[0m# ");
    p
}

fn readline(history: &mut Vec<Vec<u8>>, prompt: &[u8]) -> Option<Vec<u8>> {
    let mut line: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut hist = history.len();
    let redraw = |line: &[u8], pos: usize| {
        out(b"\r\x1b[K");
        out(prompt);
        out(line);
        if line.len() > pos {
            out(b"\x1b[");
            out(&num((line.len() - pos) as u64));
            out(b"D");
        }
    };
    loop {
        let mut c = [0u8; 1];
        if sys::read(0, &mut c) != 1 {
            return if line.is_empty() { None } else { Some(line) };
        }
        match c[0] {
            b'\r' | b'\n' => {
                out(b"\r\n");
                return Some(line);
            }
            3 => {
                out(b"^C\r\n");
                return Some(Vec::new());
            }
            8 | 127 => {
                if pos > 0 {
                    pos -= 1;
                    line.remove(pos);
                    redraw(&line, pos);
                }
            }
            b'\t' => complete(&mut line, &mut pos, prompt),
            27 => {
                let mut s = [0u8; 2];
                if sys::read(0, &mut s[..1]) != 1 || (s[0] != b'[' && s[0] != b'O') {
                    continue;
                }
                if sys::read(0, &mut s[1..]) != 1 {
                    continue;
                }
                match s[1] {
                    b'A' if hist > 0 => {
                        hist -= 1;
                        line = history[hist].clone();
                        pos = line.len();
                        redraw(&line, pos);
                    }
                    b'B' => {
                        if hist + 1 < history.len() {
                            hist += 1;
                            line = history[hist].clone();
                        } else {
                            hist = history.len();
                            line.clear();
                        }
                        pos = line.len();
                        redraw(&line, pos);
                    }
                    b'C' if pos < line.len() => {
                        pos += 1;
                        out(b"\x1b[C");
                    }
                    b'D' if pos > 0 => {
                        pos -= 1;
                        out(b"\x1b[D");
                    }
                    _ => {}
                }
            }
            ch if ch >= 32 => {
                line.insert(pos, ch);
                pos += 1;
                if pos == line.len() {
                    out(&[ch]);
                } else {
                    redraw(&line, pos);
                }
            }
            _ => {}
        }
    }
}

fn complete(line: &mut Vec<u8>, pos: &mut usize, prompt: &[u8]) {
    let start = line[..*pos].iter().rposition(|&c| c == b' ').map_or(0, |i| i + 1);
    let word = line[start..*pos].to_vec();
    let (dir, pre) = match word.iter().rposition(|&c| c == b'/') {
        Some(i) => (word[..i + 1].to_vec(), word[i + 1..].to_vec()),
        None => (Vec::new(), word.clone()),
    };
    let listdir = if dir.is_empty() { b".".to_vec() } else { dir.clone() };
    let mut m: Vec<Vec<u8>> =
        sys::list_dir(&listdir).unwrap_or_default().into_iter().filter(|c| c.starts_with(&pre)).collect();
    m.sort();
    if m.len() == 1 {
        let add = &m[0][pre.len()..];
        for (k, &c) in add.iter().enumerate() {
            line.insert(*pos + k, c);
        }
        *pos += add.len();
        line.insert(*pos, b' ');
        *pos += 1;
        out(b"\r\x1b[K");
        out(prompt);
        out(line);
    } else if m.len() > 1 {
        out(b"\r\n");
        for c in &m {
            out(c);
            out(b"  ");
        }
        out(b"\r\n");
        out(prompt);
        out(line);
    }
}

// ---------------------------------------------------------------- tokenizing

/// Split one command segment (already free of `;` `&&` `||`) into words,
/// applying quote removal, variable and command substitution, word splitting
/// and globbing. Redirection operators and `|` come back as their own words.
fn tokenize(line: &[u8], last: i32) -> Vec<Vec<u8>> {
    let mut words: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut started = false;
    let mut glob_ok = true; // false once any part of this word was quoted
    let mut i = 0;

    let flush = |words: &mut Vec<Vec<u8>>, cur: &mut Vec<u8>, started: &mut bool, glob_ok: &mut bool| {
        if *started {
            if *glob_ok && has_glob(cur) {
                let mut g = glob(cur);
                if g.is_empty() {
                    words.push(core::mem::take(cur));
                } else {
                    words.append(&mut g);
                    cur.clear();
                }
            } else {
                words.push(core::mem::take(cur));
            }
        }
        *started = false;
        *glob_ok = true;
    };

    while i < line.len() {
        let c = line[i];
        match c {
            b' ' | b'\t' => flush(&mut words, &mut cur, &mut started, &mut glob_ok),
            b'|' => {
                flush(&mut words, &mut cur, &mut started, &mut glob_ok);
                words.push(b"|".to_vec());
            }
            b'<' => {
                flush(&mut words, &mut cur, &mut started, &mut glob_ok);
                words.push(b"<".to_vec());
            }
            b'>' => {
                flush(&mut words, &mut cur, &mut started, &mut glob_ok);
                if line.get(i + 1) == Some(&b'>') {
                    words.push(b">>".to_vec());
                    i += 1;
                } else {
                    words.push(b">".to_vec());
                }
            }
            b'2' if !started && line.get(i + 1) == Some(&b'>') => {
                words.push(b"2>".to_vec());
                i += 1;
            }
            b'~' if !started && (line.get(i + 1).is_none() || line[i + 1] == b'/' || line[i + 1] == b' ') => {
                started = true;
                if let Some(h) = sched::with_proc(|p| p.getenv(b"HOME").map(|v| v.to_vec())) {
                    cur.extend_from_slice(&h);
                }
            }
            b'\'' => {
                started = true;
                glob_ok = false;
                i += 1;
                while i < line.len() && line[i] != b'\'' {
                    cur.push(line[i]);
                    i += 1;
                }
            }
            b'"' => {
                started = true;
                glob_ok = false;
                i += 1;
                while i < line.len() && line[i] != b'"' {
                    if line[i] == b'$' {
                        i += dollar(&line[i..], last, &mut cur, true, &mut words, &mut started);
                    } else if line[i] == b'\\' && i + 1 < line.len() {
                        cur.push(line[i + 1]);
                        i += 2;
                    } else {
                        cur.push(line[i]);
                        i += 1;
                    }
                }
            }
            b'$' => {
                started = true;
                i += dollar(&line[i..], last, &mut cur, false, &mut words, &mut started);
                continue;
            }
            b'\\' if i + 1 < line.len() => {
                started = true;
                glob_ok = false;
                cur.push(line[i + 1]);
                i += 1;
            }
            _ => {
                started = true;
                cur.push(c);
            }
        }
        i += 1;
    }
    flush(&mut words, &mut cur, &mut started, &mut glob_ok);
    words
}

/// Handle a `$...` at the front of `s`: `$?`, `$VAR`, `${VAR}`, or `$(cmd)`.
/// Appends to `cur`; for an unquoted expansion that contains whitespace, splits
/// into words (pushing completed ones). Returns bytes consumed from `s`.
fn dollar(s: &[u8], last: i32, cur: &mut Vec<u8>, in_dq: bool, words: &mut Vec<Vec<u8>>, started: &mut bool) -> usize {
    if s.len() < 2 {
        cur.push(b'$');
        return 1;
    }
    // Command substitution: $(...)
    if s[1] == b'(' {
        let mut depth = 1;
        let mut j = 2;
        while j < s.len() && depth > 0 {
            match s[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                break;
            }
            j += 1;
        }
        let inner = &s[2..j];
        let mut data = capture(inner, last);
        while data.last() == Some(&b'\n') {
            data.pop();
        }
        emit(&data, in_dq, cur, words, started);
        return (j + 1).min(s.len());
    }
    // $?  -> last exit status
    if s[1] == b'?' {
        cur.extend_from_slice(&num(last as u64));
        *started = true;
        return 2;
    }
    // $NAME or ${NAME}
    let (name, consumed): (&[u8], usize) = if s[1] == b'{' {
        let end = s.iter().position(|&c| c == b'}').unwrap_or(s.len());
        (&s[2..end], (end + 1).min(s.len()))
    } else {
        let mut j = 1;
        while j < s.len() && (s[j].is_ascii_alphanumeric() || s[j] == b'_') {
            j += 1;
        }
        (&s[1..j], j)
    };
    if name.is_empty() {
        cur.push(b'$');
        return 1;
    }
    let v = sched::with_proc(|p| p.getenv(name).map(|v| v.to_vec())).unwrap_or_default();
    emit(&v, in_dq, cur, words, started);
    consumed
}

/// Append expanded bytes, word-splitting on whitespace when unquoted.
fn emit(bytes: &[u8], in_dq: bool, cur: &mut Vec<u8>, words: &mut Vec<Vec<u8>>, started: &mut bool) {
    if in_dq {
        cur.extend_from_slice(bytes);
        *started = true;
        return;
    }
    let mut it = bytes.split(|&c| c == b' ' || c == b'\t' || c == b'\n').filter(|p| !p.is_empty());
    if let Some(first) = it.next() {
        cur.extend_from_slice(first);
        *started = true;
        for part in it {
            words.push(core::mem::take(cur));
            cur.extend_from_slice(part);
            *started = true;
        }
    }
}

// ---------------------------------------------------------------- globbing

fn has_glob(w: &[u8]) -> bool {
    w.iter().any(|&c| c == b'*' || c == b'?' || c == b'[')
}

/// Expand a word containing glob metacharacters in its final path component.
fn glob(pat: &[u8]) -> Vec<Vec<u8>> {
    let (dir, base): (Vec<u8>, &[u8]) = match pat.iter().rposition(|&c| c == b'/') {
        Some(i) => (pat[..i + 1].to_vec(), &pat[i + 1..]),
        None => (Vec::new(), pat),
    };
    if has_glob(&dir) {
        return Vec::new(); // only the last component is globbed
    }
    let listdir = if dir.is_empty() { b".".to_vec() } else { dir.clone() };
    let mut out = Vec::new();
    if let Ok(mut names) = sys::list_dir(&listdir) {
        names.sort();
        for n in names {
            // A leading dot is only matched by an explicit leading dot.
            if n.first() == Some(&b'.') && base.first() != Some(&b'.') {
                continue;
            }
            if fnmatch(base, &n) {
                let mut full = dir.clone();
                full.extend_from_slice(&n);
                out.push(full);
            }
        }
    }
    out
}

/// Shell wildcard match: `*` any run, `?` one char, `[abc]`/`[a-z]`/`[!..]`.
fn fnmatch(pat: &[u8], name: &[u8]) -> bool {
    let (mut p, mut n) = (0, 0);
    let (mut star_p, mut star_n) = (usize::MAX, 0);
    while n < name.len() {
        if p < pat.len() {
            match pat[p] {
                b'*' => {
                    star_p = p;
                    star_n = n;
                    p += 1;
                    continue;
                }
                b'?' => {
                    p += 1;
                    n += 1;
                    continue;
                }
                b'[' => {
                    if let Some((ok, np)) = class(&pat[p..], name[n]) {
                        if ok {
                            p += np;
                            n += 1;
                            continue;
                        }
                    } else if pat[p] == name[n] {
                        p += 1;
                        n += 1;
                        continue;
                    }
                }
                c if c == name[n] => {
                    p += 1;
                    n += 1;
                    continue;
                }
                _ => {}
            }
        }
        // Mismatch: backtrack to the last '*', if any.
        if star_p != usize::MAX {
            star_n += 1;
            n = star_n;
            p = star_p + 1;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

/// Match a `[...]` class against `ch`; returns (matched, bytes-of-class) or
/// None if `pat` is not a well-formed class.
fn class(pat: &[u8], ch: u8) -> Option<(bool, usize)> {
    if pat.first() != Some(&b'[') {
        return None;
    }
    let mut i = 1;
    let neg = pat.get(1) == Some(&b'!') || pat.get(1) == Some(&b'^');
    if neg {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pat.len() {
        if pat[i] == b']' && !first {
            return Some((matched ^ neg, i + 1));
        }
        first = false;
        if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
            if pat[i] <= ch && ch <= pat[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if pat[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    None // no closing ']'
}

// ---------------------------------------------------------------- pipelines

struct Stage {
    argv: Vec<Vec<u8>>,
    redir_in: Option<Vec<u8>>,
    redir_out: Option<(Vec<u8>, bool)>, // (path, append)
    redir_err: Option<Vec<u8>>,
}

/// Pull redirections out of a stage's words.
fn parse_stage(words: &[Vec<u8>]) -> Option<Stage> {
    let mut st = Stage { argv: Vec::new(), redir_in: None, redir_out: None, redir_err: None };
    let mut i = 0;
    while i < words.len() {
        match words[i].as_slice() {
            b">" | b">>" | b"<" | b"2>" => {
                let target = words.get(i + 1)?.clone();
                match words[i].as_slice() {
                    b">" => st.redir_out = Some((target, false)),
                    b">>" => st.redir_out = Some((target, true)),
                    b"<" => st.redir_in = Some(target),
                    _ => st.redir_err = Some(target),
                }
                i += 2;
            }
            _ => {
                st.argv.push(words[i].clone());
                i += 1;
            }
        }
    }
    if st.argv.is_empty() && st.redir_out.is_none() && st.redir_in.is_none() {
        return None;
    }
    Some(st)
}

/// Make fd `dst` a duplicate of fd `src`, properly releasing whatever `dst`
/// held (so a pipe's writer/reader count stays correct).
fn redir(dst: i32, src: i32) {
    sys::close(dst);
    sys::dup2(src, dst);
}

/// Apply a stage's explicit `< > >> 2>` redirections over fds 0/1/2.
fn apply_redir(st: &Stage) {
    if let Some(p) = &st.redir_in {
        let f = sys::open(p, sys::O_RDONLY, 0);
        if f >= 0 {
            redir(0, f);
            sys::close(f);
        } else {
            perr(b"", p, sys::ENOENT);
        }
    }
    if let Some((p, append)) = &st.redir_out {
        let flags = sys::O_WRONLY | sys::O_CREAT | if *append { sys::O_APPEND } else { sys::O_TRUNC };
        let f = sys::open(p, flags, 0o644);
        if f >= 0 {
            redir(1, f);
            sys::close(f);
        }
    }
    if let Some(p) = &st.redir_err {
        let f = sys::open(p, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644);
        if f >= 0 {
            redir(2, f);
            sys::close(f);
        }
    }
}

fn exec_stage(st: &Stage, last: i32) -> i32 {
    if st.argv.is_empty() {
        return 0; // pure redirection, e.g. `> file`
    }
    if let Some(code) = builtin(&st.argv, last) {
        return code;
    }
    let a: Vec<&[u8]> = st.argv.iter().map(|v| &v[..]).collect();
    match applets::find(&st.argv[0]) {
        Some(f) => {
            sched::with_proc(|p| p.argv = st.argv.clone());
            f(&a)
        }
        None => {
            out(&st.argv[0]);
            out(b": not found\n");
            127
        }
    }
}

/// Run a pipeline. Each non-final stage writes into an unbounded RAM pipe that
/// the next stage drains; stdio is saved and restored around the whole thing.
fn run_pipeline(stages: Vec<Stage>, last: i32) -> i32 {
    let n = stages.len();
    let save0 = sys::dup_high(0, 20);
    let save1 = sys::dup_high(1, 20);
    let save2 = sys::dup_high(2, 20);
    let mut prev_read = -1i32;
    let mut code = 0;
    for (i, st) in stages.iter().enumerate() {
        let is_last = i + 1 == n;
        // stdin: previous pipe, or the terminal.
        if prev_read >= 0 {
            redir(0, prev_read);
            sys::close(prev_read);
            prev_read = -1;
        } else {
            redir(0, save0);
        }
        // stdout: a fresh pipe for the next stage, or the terminal. Setting it
        // up closes the previous stage's write end, so its reader sees EOF.
        if !is_last {
            let mut pf = [-1, -1];
            sys::pipe(&mut pf);
            redir(1, pf[1]);
            sys::close(pf[1]);
            prev_read = pf[0];
        } else {
            redir(1, save1);
        }
        redir(2, save2);
        apply_redir(st);
        code = exec_stage(st, if i == 0 { last } else { code });
    }
    // Restore the terminal and drop the saved copies.
    redir(0, save0);
    redir(1, save1);
    redir(2, save2);
    sys::close(save0);
    sys::close(save1);
    sys::close(save2);
    if prev_read >= 0 {
        sys::close(prev_read);
    }
    code
}

/// Run `inner` as a command line, returning everything it writes to stdout.
fn capture(inner: &[u8], last: i32) -> Vec<u8> {
    let mut pf = [-1, -1];
    sys::pipe(&mut pf);
    let save1 = sys::dup_high(1, 20);
    redir(1, pf[1]);
    sys::close(pf[1]);
    run_line(inner, last);
    redir(1, save1); // closes the pipe writer -> reader can hit EOF
    sys::close(save1);
    let data = read_fd(pf[0]);
    sys::close(pf[0]);
    data
}

// ---------------------------------------------------------------- builtins

fn builtin(argv: &[Vec<u8>], _last: i32) -> Option<i32> {
    let a: Vec<&[u8]> = argv.iter().map(|v| &v[..]).collect();
    Some(match a[0] {
        b"cd" => {
            let dir = a.get(1).copied().unwrap_or(b"/root");
            if sys::chdir(dir) < 0 {
                msg(b"cd", b"no such directory");
                1
            } else {
                let mut buf = [0u8; 512];
                let n = sys::getcwd(&mut buf);
                if n > 1 {
                    sched::with_proc(|p| p.setenv(b"PWD", &buf[..n as usize - 1]));
                }
                0
            }
        }
        b"export" | b"set" => {
            for s in &a[1..] {
                if let Some(eq) = s.iter().position(|&c| c == b'=') {
                    sched::with_proc(|p| p.setenv(&s[..eq], &s[eq + 1..]));
                }
            }
            0
        }
        b"unset" => {
            sched::with_proc(|p| {
                for s in &a[1..] {
                    p.env.retain(|(n, _)| n != s);
                }
            });
            0
        }
        b"exit" => sys::exit(a.get(1).and_then(|s| parse_u64(s)).map_or(0, |n| n as i32)),
        b":" => 0,
        b"help" => {
            out(b"teenyOS core shell -- it runs on the firmware; there is no kernel.\n\n");
            out(b"Builtins:  cd  export  unset  exit  help  :\n");
            out(b"Commands:  ls cat echo cp mv rm mkdir rmdir ln touch pwd chmod stat\n");
            out(b"           grep wc head tail sort uniq tee seq basename dirname hexdump\n");
            out(b"           test [ true false clear uname date free ps env net wget persist\n");
            out(b"Operators: ;  &&  ||  |  >  >>  <  2>\n");
            out(b"Expansion: $VAR  ${VAR}  $?  $(command)  *  ?  [a-z]  ~\n");
            0
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------- top level

#[derive(PartialEq, Clone, Copy)]
enum Op {
    None,
    Semi,
    And,
    Or,
}

/// Split a line on `;` `&&` `||`, ignoring operators inside quotes or `$(...)`.
fn split_ops(line: &[u8]) -> Vec<(Vec<u8>, Op)> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut op = Op::None;
    let mut q = 0u8;
    let mut depth = 0i32;
    let mut i = 0;
    while i < line.len() {
        let c = line[i];
        if q != 0 {
            cur.push(c);
            if c == q {
                q = 0;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => {
                q = c;
                cur.push(c);
            }
            b'(' => {
                depth += 1;
                cur.push(c);
            }
            b')' => {
                depth -= 1;
                cur.push(c);
            }
            b';' if depth == 0 => {
                out.push((core::mem::take(&mut cur), op));
                op = Op::Semi;
            }
            b'&' if depth == 0 && line.get(i + 1) == Some(&b'&') => {
                out.push((core::mem::take(&mut cur), op));
                op = Op::And;
                i += 1;
            }
            b'|' if depth == 0 && line.get(i + 1) == Some(&b'|') => {
                out.push((core::mem::take(&mut cur), op));
                op = Op::Or;
                i += 1;
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    out.push((cur, op));
    out
}

fn run_line(line: &[u8], last: i32) -> i32 {
    let mut code = last;
    for (seg, op_before) in split_ops(line) {
        let skip = match op_before {
            Op::And => code != 0,
            Op::Or => code == 0,
            _ => false,
        };
        if skip {
            continue;
        }
        let words = tokenize(&seg, code);
        if words.is_empty() {
            continue;
        }
        // Split words on `|` into pipeline stages.
        let mut stages = Vec::new();
        let mut cur = Vec::new();
        for w in words {
            if w == b"|" {
                if let Some(s) = parse_stage(&cur) {
                    stages.push(s);
                }
                cur = Vec::new();
            } else {
                cur.push(w);
            }
        }
        if let Some(s) = parse_stage(&cur) {
            stages.push(s);
        }
        if !stages.is_empty() {
            code = run_pipeline(stages, code);
        }
    }
    code
}

/// A boot-time regression harness, compiled only under the `selftest` feature.
/// It drives the real parser and executor and writes labelled output to
/// /root/RESULTS, which persistence flushes to the boot disk for the host test
/// harness to read back. It ships in no release build.
#[cfg(feature = "selftest")]
pub fn selftest() {
    fn label(l: &[u8]) {
        let f = sys::open(b"/root/RESULTS", sys::O_WRONLY | sys::O_CREAT | sys::O_APPEND, 0o644);
        if f >= 0 {
            sys::write(f, b"=== ");
            sys::write(f, l);
            sys::write(f, b" ===\n");
            sys::close(f);
        }
    }
    // (label, command). Each command appends its own stdout to /root/RESULTS.
    let tests: &[(&[u8], &[u8])] = &[
        (b"pipe", b"seq 5 | head -3 | wc -l >> /root/RESULTS"),
        (b"pipe-sort", b"seq 5 | sort -r | head -1 >> /root/RESULTS"),
        (b"subst", b"echo $(seq 3) >> /root/RESULTS"),
        (b"subst-nested", b"echo $(echo $(seq 2)) >> /root/RESULTS"),
        (b"var", b"export X=hi; echo $X-$X >> /root/RESULTS"),
        (b"status-false", b"false; echo $? >> /root/RESULTS"),
        (b"status-true", b"true; echo $? >> /root/RESULTS"),
        (b"and", b"true && echo A >> /root/RESULTS"),
        (b"or", b"false || echo B >> /root/RESULTS"),
        (b"and-skip", b"false && echo C >> /root/RESULTS; echo D >> /root/RESULTS"),
        (b"quote-dq", b"echo \"a b\"   c >> /root/RESULTS"),
        (b"quote-sq", b"echo 'x  y' >> /root/RESULTS"),
        (b"redir-append", b"echo one >> /root/RESULTS; echo two >> /root/RESULTS"),
        (b"glob-setup", b"mkdir /root/g; touch /root/g/a.txt; touch /root/g/b.txt; touch /root/g/c.dat"),
        (b"glob-star", b"cd /root/g; echo *.txt >> /root/RESULTS; cd /root"),
        (b"glob-class", b"echo /root/g/[ab].txt >> /root/RESULTS"),
        (b"glob-none", b"echo /root/g/*.zzz >> /root/RESULTS"),
    ];
    for (l, cmd) in tests {
        label(l);
        run_line(cmd, 0);
    }
    label(b"DONE");
    crate::persist::save();
}

pub fn main() {
    out(b"\r\n  \x1b[1;36mteenyOS core\x1b[0m -- a desktop OS with no kernel.\r\n");
    out(b"  Running on the firmware. Type 'help'. Try: ls, net, wget, persist.\r\n\r\n");
    let mut history: Vec<Vec<u8>> = Vec::new();
    let mut last = 0;
    loop {
        let prompt = build_prompt(last);
        out(&prompt);
        let Some(line) = readline(&mut history, &prompt) else {
            sys::sleep_ms(50);
            continue;
        };
        if trim(&line).is_empty() {
            continue;
        }
        if history.last().map(|h| h.as_slice()) != Some(line.as_slice()) {
            history.push(line.clone());
            if history.len() > 100 {
                history.remove(0);
            }
        }
        last = run_line(&line, last);
    }
}
