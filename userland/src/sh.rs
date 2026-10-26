// The shell. A small POSIX-flavoured sh: quoting, variables, $(...), $((...)),
// globbing, pipes, redirections, && || ; &, if/while/until/for, and an
// interactive line editor with history and tab completion.

use crate::rt::{self, flush, out};
use crate::sys;
use crate::util::*;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------- lexing

#[derive(Clone)]
enum Part {
    Lit(Vec<u8>),
    Quoted(Vec<u8>),
    Var(Vec<u8>, bool),
    Cmd(Vec<u8>, bool),
    Arith(Vec<u8>, bool),
}

type Word = Vec<Part>;

#[derive(Clone)]
enum Tok {
    Word(Word),
    Op(&'static [u8]),
    Newline,
}

enum ParseErr {
    Incomplete,
    Syntax(&'static [u8]),
}

const OPS: &[&[u8]] = &[
    b"2>&1", b"2>>", b">&2", b"&&", b"||", b">>", b"2>", b"|", b"&", b";", b">", b"<",
];

fn is_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Find the end of a "$(" ... ")" or "$((" ... "))" body starting at `i`
/// (just past the opening parens). Returns the index of the closing paren.
fn match_paren(s: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1;
    let mut q = 0u8;
    while i < s.len() {
        let c = s[i];
        if q != 0 {
            if c == q {
                q = 0;
            } else if c == b'\\' && q == b'"' {
                i += 1;
            }
        } else if c == b'\'' || c == b'"' {
            q = c;
        } else if c == b'\\' {
            i += 1;
        } else if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Parse a "$..." expansion at s[i] == '$'. Returns the part and new index.
fn lex_dollar(s: &[u8], i: usize, quoted: bool) -> Result<(Part, usize), ParseErr> {
    let n = s.get(i + 1).copied().unwrap_or(0);
    if n == b'(' {
        if s.get(i + 2) == Some(&b'(') {
            let end = match_paren(s, i + 3).ok_or(ParseErr::Incomplete)?;
            if s.get(end + 1) != Some(&b')') {
                return Err(ParseErr::Syntax(b"bad $(( ))"));
            }
            return Ok((Part::Arith(s[i + 3..end].to_vec(), quoted), end + 2));
        }
        let end = match_paren(s, i + 2).ok_or(ParseErr::Incomplete)?;
        return Ok((Part::Cmd(s[i + 2..end].to_vec(), quoted), end + 1));
    }
    if n == b'{' {
        let end = s[i..]
            .iter()
            .position(|&c| c == b'}')
            .ok_or(ParseErr::Incomplete)?
            + i;
        return Ok((Part::Var(s[i + 2..end].to_vec(), quoted), end + 1));
    }
    if b"?$#!@*".contains(&n) || n.is_ascii_digit() {
        return Ok((Part::Var(vec![n], quoted), i + 2));
    }
    let mut j = i + 1;
    while j < s.len() && is_name(s[j]) {
        j += 1;
    }
    if j == i + 1 {
        return Ok((Part::Lit(b"$".to_vec()), i + 1));
    }
    Ok((Part::Var(s[i + 1..j].to_vec(), quoted), j))
}

fn lex(s: &[u8]) -> Result<Vec<Tok>, ParseErr> {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        if c == b'\n' {
            toks.push(Tok::Newline);
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < s.len() && s[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'\\' && s.get(i + 1) == Some(&b'\n') {
            i += 2;
            continue;
        }
        if let Some(op) = OPS.iter().find(|op| s[i..].starts_with(op)) {
            toks.push(Tok::Op(op));
            i += op.len();
            continue;
        }
        // A word: runs until unquoted whitespace or operator.
        let mut w: Word = Vec::new();
        let mut lit = Vec::new();
        if c == b'~' && (i + 1 == s.len() || b"/ \t\n;|&".contains(&s[i + 1])) {
            w.push(Part::Var(b"HOME".to_vec(), true));
            i += 1;
        }
        while i < s.len() {
            let c = s[i];
            if b" \t\r\n;|&<>".contains(&c) {
                break;
            }
            match c {
                b'\\' => {
                    if i + 1 >= s.len() {
                        return Err(ParseErr::Incomplete);
                    }
                    if s[i + 1] != b'\n' {
                        flush_lit(&mut w, &mut lit);
                        w.push(Part::Quoted(vec![s[i + 1]]));
                    }
                    i += 2;
                }
                b'\'' => {
                    let end = s[i + 1..]
                        .iter()
                        .position(|&c| c == b'\'')
                        .ok_or(ParseErr::Incomplete)?
                        + i
                        + 1;
                    flush_lit(&mut w, &mut lit);
                    w.push(Part::Quoted(s[i + 1..end].to_vec()));
                    i = end + 1;
                }
                b'"' => {
                    flush_lit(&mut w, &mut lit);
                    let mut q = Vec::new();
                    i += 1;
                    loop {
                        if i >= s.len() {
                            return Err(ParseErr::Incomplete);
                        }
                        let c = s[i];
                        if c == b'"' {
                            i += 1;
                            break;
                        }
                        if c == b'\\' && i + 1 < s.len() && b"\"\\$`\n".contains(&s[i + 1]) {
                            if s[i + 1] != b'\n' {
                                q.push(s[i + 1]);
                            }
                            i += 2;
                        } else if c == b'$' {
                            let (p, j) = lex_dollar(s, i, true)?;
                            if let Part::Lit(l) = p {
                                q.extend_from_slice(&l);
                            } else {
                                w.push(Part::Quoted(core::mem::take(&mut q)));
                                w.push(p);
                            }
                            i = j;
                        } else {
                            q.push(c);
                            i += 1;
                        }
                    }
                    w.push(Part::Quoted(q));
                }
                b'$' => {
                    let (p, j) = lex_dollar(s, i, false)?;
                    if let Part::Lit(l) = p {
                        lit.extend_from_slice(&l);
                    } else {
                        flush_lit(&mut w, &mut lit);
                        w.push(p);
                    }
                    i = j;
                }
                _ => {
                    lit.push(c);
                    i += 1;
                }
            }
        }
        flush_lit(&mut w, &mut lit);
        toks.push(Tok::Word(w));
    }
    Ok(toks)
}

fn flush_lit(w: &mut Word, lit: &mut Vec<u8>) {
    if !lit.is_empty() {
        w.push(Part::Lit(core::mem::take(lit)));
    }
}

// ---------------------------------------------------------------- parsing

#[derive(Clone, Copy, PartialEq)]
enum RKind {
    In,
    Out,
    Append,
    Dup,
}

#[derive(Clone)]
struct Redir {
    fd: i32,
    kind: RKind,
    target: Word,
}

#[derive(Clone)]
enum Node {
    Cmd {
        assigns: Vec<(Vec<u8>, Word)>,
        words: Vec<Word>,
        redirs: Vec<Redir>,
    },
    Pipe(Vec<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    Seq(Vec<Node>),
    Bg(Box<Node>),
    If(Vec<(Node, Node)>, Option<Box<Node>>),
    Loop(Box<Node>, Box<Node>, bool),
    For(Vec<u8>, Option<Vec<Word>>, Box<Node>),
}

/// The plain text of a word, if it is a single unquoted literal.
fn bare(w: &Word) -> Option<&[u8]> {
    match w.as_slice() {
        [Part::Lit(l)] => Some(l),
        _ => None,
    }
}

const KEYWORDS: &[&[u8]] = &[b"then", b"elif", b"else", b"fi", b"do", b"done", b"in"];

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek_word(&self) -> Option<&[u8]> {
        match self.t.get(self.i) {
            Some(Tok::Word(w)) => bare(w),
            _ => None,
        }
    }
    fn skip_newlines(&mut self) {
        while matches!(self.t.get(self.i), Some(Tok::Newline) | Some(Tok::Op(b";"))) {
            self.i += 1;
        }
    }
    fn expect(&mut self, kw: &[u8]) -> Result<(), ParseErr> {
        self.skip_newlines();
        match self.peek_word() {
            Some(w) if w == kw => {
                self.i += 1;
                Ok(())
            }
            _ if self.i >= self.t.len() => Err(ParseErr::Incomplete),
            _ => Err(ParseErr::Syntax(b"unexpected token")),
        }
    }

    /// Commands separated by ; & or newlines, until EOF or a closing keyword.
    fn list(&mut self) -> Result<Node, ParseErr> {
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.i >= self.t.len() {
                break;
            }
            if let Some(w) = self.peek_word() {
                if KEYWORDS.contains(&w) {
                    break;
                }
            }
            let n = self.and_or()?;
            if let Some(Tok::Op(b"&")) = self.t.get(self.i) {
                // "&" both backgrounds and separates.
                self.i += 1;
                items.push(Node::Bg(Box::new(n)));
                continue;
            }
            items.push(n);
            match self.t.get(self.i) {
                None | Some(Tok::Newline) | Some(Tok::Op(b";")) => {}
                Some(Tok::Word(w)) if bare(w).map_or(false, |w| KEYWORDS.contains(&w)) => {}
                _ => return Err(ParseErr::Syntax(b"unexpected token")),
            }
        }
        Ok(Node::Seq(items))
    }

    fn and_or(&mut self) -> Result<Node, ParseErr> {
        let mut left = self.pipeline()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Tok::Op(b"&&")) => true,
                Some(Tok::Op(b"||")) => false,
                _ => return Ok(left),
            };
            self.i += 1;
            while let Some(Tok::Newline) = self.t.get(self.i) {
                self.i += 1;
            }
            if self.i >= self.t.len() {
                return Err(ParseErr::Incomplete);
            }
            let right = self.pipeline()?;
            left = if op {
                Node::And(Box::new(left), Box::new(right))
            } else {
                Node::Or(Box::new(left), Box::new(right))
            };
        }
    }

    fn pipeline(&mut self) -> Result<Node, ParseErr> {
        let neg = self.peek_word() == Some(b"!");
        if neg {
            self.i += 1;
        }
        let mut cmds = vec![self.command()?];
        while let Some(Tok::Op(b"|")) = self.t.get(self.i) {
            self.i += 1;
            while let Some(Tok::Newline) = self.t.get(self.i) {
                self.i += 1;
            }
            if self.i >= self.t.len() {
                return Err(ParseErr::Incomplete);
            }
            cmds.push(self.command()?);
        }
        let n = if cmds.len() == 1 {
            cmds.pop().unwrap()
        } else {
            Node::Pipe(cmds)
        };
        Ok(if neg { Node::Not(Box::new(n)) } else { n })
    }

    fn command(&mut self) -> Result<Node, ParseErr> {
        match self.peek_word() {
            Some(b"if") => {
                self.i += 1;
                let mut arms = Vec::new();
                let mut els = None;
                loop {
                    let cond = self.list()?;
                    self.expect(b"then")?;
                    let body = self.list()?;
                    arms.push((cond, body));
                    self.skip_newlines();
                    match self.peek_word() {
                        Some(b"elif") => self.i += 1,
                        Some(b"else") => {
                            self.i += 1;
                            els = Some(Box::new(self.list()?));
                            self.expect(b"fi")?;
                            break;
                        }
                        Some(b"fi") => {
                            self.i += 1;
                            break;
                        }
                        _ if self.i >= self.t.len() => return Err(ParseErr::Incomplete),
                        _ => return Err(ParseErr::Syntax(b"expected fi")),
                    }
                }
                Ok(Node::If(arms, els))
            }
            Some(kw @ (b"while" | b"until")) => {
                let until = kw == b"until";
                self.i += 1;
                let cond = self.list()?;
                self.expect(b"do")?;
                let body = self.list()?;
                self.expect(b"done")?;
                Ok(Node::Loop(Box::new(cond), Box::new(body), until))
            }
            Some(b"for") => {
                self.i += 1;
                let var = match self.peek_word() {
                    Some(v) => v.to_vec(),
                    None if self.i >= self.t.len() => return Err(ParseErr::Incomplete),
                    None => return Err(ParseErr::Syntax(b"bad for")),
                };
                self.i += 1;
                let mut list = None;
                while let Some(Tok::Newline) = self.t.get(self.i) {
                    self.i += 1;
                }
                if self.peek_word() == Some(b"in") {
                    self.i += 1;
                    let mut ws = Vec::new();
                    while let Some(Tok::Word(w)) = self.t.get(self.i) {
                        ws.push(w.clone());
                        self.i += 1;
                    }
                    list = Some(ws);
                }
                self.expect(b"do")?;
                let body = self.list()?;
                self.expect(b"done")?;
                Ok(Node::For(var, list, Box::new(body)))
            }
            _ => self.simple(),
        }
    }

    fn simple(&mut self) -> Result<Node, ParseErr> {
        let (mut assigns, mut words, mut redirs) = (Vec::new(), Vec::new(), Vec::new());
        loop {
            match self.t.get(self.i) {
                Some(Tok::Word(w)) => {
                    // NAME=value before the command name is an assignment.
                    if words.is_empty() {
                        if let Some(Part::Lit(l)) = w.first() {
                            if let Some(eq) = l.iter().position(|&c| c == b'=') {
                                if eq > 0
                                    && l[..eq].iter().all(|&c| is_name(c))
                                    && !l[0].is_ascii_digit()
                                {
                                    let mut val: Word = w[1..].to_vec();
                                    if eq + 1 < l.len() {
                                        val.insert(0, Part::Lit(l[eq + 1..].to_vec()));
                                    }
                                    assigns.push((l[..eq].to_vec(), val));
                                    self.i += 1;
                                    continue;
                                }
                            }
                        }
                    }
                    words.push(w.clone());
                    self.i += 1;
                }
                Some(Tok::Op(op))
                    if [b">" as &[u8], b">>", b"<", b"2>", b"2>>", b"2>&1", b">&2"]
                        .contains(op) =>
                {
                    let op: &[u8] = op;
                    self.i += 1;
                    let (fd, kind) = match op {
                        b">" => (1, RKind::Out),
                        b">>" => (1, RKind::Append),
                        b"<" => (0, RKind::In),
                        b"2>" => (2, RKind::Out),
                        b"2>>" => (2, RKind::Append),
                        b"2>&1" => (2, RKind::Dup),
                        _ => (1, RKind::Dup),
                    };
                    let target = if kind == RKind::Dup {
                        vec![Part::Lit(if fd == 2 {
                            b"1".to_vec()
                        } else {
                            b"2".to_vec()
                        })]
                    } else {
                        match self.t.get(self.i) {
                            Some(Tok::Word(w)) => {
                                self.i += 1;
                                w.clone()
                            }
                            None => return Err(ParseErr::Incomplete),
                            _ => return Err(ParseErr::Syntax(b"missing redirect target")),
                        }
                    };
                    redirs.push(Redir { fd, kind, target });
                }
                _ => break,
            }
        }
        if assigns.is_empty() && words.is_empty() && redirs.is_empty() {
            return Err(if self.i >= self.t.len() {
                ParseErr::Incomplete
            } else {
                ParseErr::Syntax(b"unexpected token")
            });
        }
        Ok(Node::Cmd {
            assigns,
            words,
            redirs,
        })
    }
}

fn parse(src: &[u8]) -> Result<Node, ParseErr> {
    let t = lex(src)?;
    let mut p = Parser { t, i: 0 };
    let n = p.list()?;
    if p.i < p.t.len() {
        return Err(ParseErr::Syntax(b"unexpected keyword"));
    }
    Ok(n)
}

// ---------------------------------------------------------------- glob

fn glob_match(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob_match(&p[1..], s) || (!s.is_empty() && glob_match(p, &s[1..])),
        (Some(b'?'), Some(_)) => glob_match(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => glob_match(&p[1..], &s[1..]),
        _ => false,
    }
}

/// Expand wildcards in the last path component. No match: the pattern itself.
fn glob(pat: &[u8]) -> Vec<Vec<u8>> {
    let (dir, base) = match pat.iter().rposition(|&c| c == b'/') {
        Some(i) => (&pat[..i + 1], &pat[i + 1..]),
        None => (&b""[..], pat),
    };
    if dir.contains(&b'*') || dir.contains(&b'?') {
        return vec![pat.to_vec()];
    }
    let list = if dir.is_empty() { b"." as &[u8] } else { dir };
    let mut m: Vec<Vec<u8>> = list_dir(list)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| (n[0] != b'.' || base.first() == Some(&b'.')) && glob_match(base, n))
        .map(|n| {
            let mut p = dir.to_vec();
            p.extend_from_slice(&n);
            p
        })
        .collect();
    if m.is_empty() {
        return vec![pat.to_vec()];
    }
    m.sort();
    m
}

// ---------------------------------------------------------------- arithmetic

struct Arith<'a> {
    s: &'a [u8],
    i: usize,
    sh: &'a Shell,
}

impl Arith<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, op: &[u8]) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(op) {
            self.i += op.len();
            true
        } else {
            false
        }
    }
    fn cmp(&mut self) -> i64 {
        let a = self.add();
        for (op, f) in [
            (b"==" as &[u8], (|x, y| x == y) as fn(i64, i64) -> bool),
            (b"!=", |x, y| x != y),
            (b"<=", |x, y| x <= y),
            (b">=", |x, y| x >= y),
            (b"<", |x, y| x < y),
            (b">", |x, y| x > y),
        ] {
            if self.eat(op) {
                let b = self.add();
                return f(a, b) as i64;
            }
        }
        a
    }
    fn add(&mut self) -> i64 {
        let mut a = self.mul();
        loop {
            if self.eat(b"+") {
                a = a.wrapping_add(self.mul());
            } else if self.eat(b"-") {
                a = a.wrapping_sub(self.mul());
            } else {
                return a;
            }
        }
    }
    fn mul(&mut self) -> i64 {
        let mut a = self.unary();
        loop {
            if self.eat(b"*") {
                a = a.wrapping_mul(self.unary());
            } else if self.eat(b"/") {
                let b = self.unary();
                a = if b == 0 { 0 } else { a / b };
            } else if self.eat(b"%") {
                let b = self.unary();
                a = if b == 0 { 0 } else { a % b };
            } else {
                return a;
            }
        }
    }
    fn unary(&mut self) -> i64 {
        if self.eat(b"-") {
            return -self.unary();
        }
        if self.eat(b"(") {
            let v = self.cmp();
            self.eat(b")");
            return v;
        }
        self.ws();
        let st = self.i;
        if self.eat(b"$") {
            return self.unary();
        }
        while self.i < self.s.len() && is_name(self.s[self.i]) {
            self.i += 1;
        }
        let tok = &self.s[st..self.i];
        if let Some(n) = parse_i64(tok) {
            return n;
        }
        self.sh
            .get(tok)
            .and_then(|v| parse_i64(trim(v)))
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------- execution

struct Var {
    name: Vec<u8>,
    val: Vec<u8>,
    export: bool,
}

pub struct Shell {
    vars: Vec<Var>,
    pos: Vec<Vec<u8>>,
    last: i32,
    last_bg: isize,
    interactive: bool,
    // In a background job: keep Ctrl-C ignored across exec.
    bg: bool,
    // Set when a foreground child died from Ctrl-C: abandon the current line.
    intr: bool,
    exit_req: Option<i32>,
    brk: u32,
    cont: bool,
    history: Vec<Vec<u8>>,
}

const BUILTINS: &[&[u8]] = &[
    b"cd",
    b"exit",
    b"export",
    b"unset",
    b"set",
    b"read",
    b"shift",
    b".",
    b"source",
    b"exec",
    b"wait",
    b"history",
    b"help",
    b"break",
    b"continue",
    b":",
    b"echo",
    b"test",
    b"[",
    b"true",
    b"false",
    b"pwd",
];

impl Shell {
    fn get(&self, name: &[u8]) -> Option<&[u8]> {
        self.vars
            .iter()
            .find(|v| v.name == name)
            .map(|v| &v.val[..])
    }

    fn set(&mut self, name: &[u8], val: &[u8], export: bool) {
        if let Some(v) = self.vars.iter_mut().find(|v| v.name == name) {
            v.val = val.to_vec();
            v.export |= export;
        } else {
            self.vars.push(Var {
                name: name.to_vec(),
                val: val.to_vec(),
                export,
            });
        }
    }

    fn special(&self, name: &[u8]) -> Option<Vec<u8>> {
        Some(match name {
            b"?" => num(self.last as u64),
            b"$" => num(sys::getpid() as u64),
            b"!" => num(self.last_bg.max(0) as u64),
            b"#" => num(self.pos.len().saturating_sub(1) as u64),
            b"@" | b"*" => self.pos.get(1..).unwrap_or(&[]).join(&b' '),
            [d] if d.is_ascii_digit() => self
                .pos
                .get((d - b'0') as usize)
                .cloned()
                .unwrap_or_default(),
            _ => return None,
        })
    }

    fn lookup(&self, name: &[u8]) -> Vec<u8> {
        // ${VAR:-default}
        if let Some(i) = find(name, b":-") {
            let v = self.lookup(&name[..i]);
            return if v.is_empty() {
                name[i + 2..].to_vec()
            } else {
                v
            };
        }
        self.special(name)
            .unwrap_or_else(|| self.get(name).unwrap_or(b"").to_vec())
    }

    fn capture(&mut self, src: &[u8]) -> Vec<u8> {
        let mut p = [0i32; 2];
        if sys::pipe(&mut p) < 0 {
            return Vec::new();
        }
        flush();
        let pid = sys::fork();
        if pid == 0 {
            sys::close(p[0]);
            sys::dup2(p[1], 1);
            sys::close(p[1]);
            self.interactive = false;
            let code = self.run_src(src);
            flush();
            sys::exit(code);
        }
        sys::close(p[1]);
        let mut v = read_fd(p[0]);
        sys::close(p[0]);
        let (_, st) = sys::wait4(pid, 0);
        self.last = sys::status_code(st);
        while v.last() == Some(&b'\n') {
            v.pop();
        }
        v
    }

    /// Expand one word into zero or more fields.
    fn expand(&mut self, w: &Word) -> Vec<Vec<u8>> {
        let mut fields: Vec<(Vec<u8>, bool)> = Vec::new();
        let mut cur = Vec::new();
        let mut started = false;
        let mut globbable = false;
        for p in w {
            let (val, quoted) = match p {
                Part::Lit(l) => {
                    globbable |= l.contains(&b'*') || l.contains(&b'?');
                    cur.extend_from_slice(l);
                    continue;
                }
                Part::Quoted(q) => {
                    cur.extend_from_slice(q);
                    started = true;
                    continue;
                }
                Part::Var(n, q) => (self.lookup(n), *q),
                Part::Cmd(src, q) => (self.capture(src), *q),
                Part::Arith(src, q) => {
                    let src = src.clone();
                    let v = Arith {
                        s: &src,
                        i: 0,
                        sh: self,
                    }
                    .cmp();
                    (
                        if v < 0 {
                            [b"-".to_vec(), num(v.unsigned_abs())].concat()
                        } else {
                            num(v as u64)
                        },
                        *q,
                    )
                }
            };
            if quoted {
                cur.extend_from_slice(&val);
                started = true;
                continue;
            }
            // Unquoted expansion results are split on whitespace.
            for &c in &val {
                if c.is_ascii_whitespace() {
                    if started || !cur.is_empty() {
                        fields.push((core::mem::take(&mut cur), globbable));
                        started = false;
                        globbable = false;
                    }
                } else {
                    cur.push(c);
                }
            }
        }
        if started || !cur.is_empty() {
            fields.push((cur, globbable));
        }
        let mut res = Vec::new();
        for (f, g) in fields {
            if g {
                res.extend(glob(&f));
            } else {
                res.push(f);
            }
        }
        res
    }

    fn expand_one(&mut self, w: &Word) -> Vec<u8> {
        self.expand(w).join(&b' ')
    }

    fn envp(&self, extra: &[(Vec<u8>, Vec<u8>)]) -> Vec<Vec<u8>> {
        let mut e = Vec::new();
        for (n, v) in extra {
            e.push([&n[..], b"=", &v[..], b"\0"].concat());
        }
        for v in &self.vars {
            if v.export && !extra.iter().any(|(n, _)| *n == v.name) {
                e.push([&v.name[..], b"=", &v.val[..], b"\0"].concat());
            }
        }
        e
    }

    fn find_cmd(&self, cmd: &[u8]) -> Option<Vec<u8>> {
        if cmd.contains(&b'/') {
            return Some(cmd.to_vec());
        }
        let path = self.get(b"PATH").unwrap_or(b"/bin:/sbin");
        for dir in path.split(|&c| c == b':') {
            let p = join(dir, cmd);
            if let Some(st) = sys::stat(&p) {
                if !st.is_dir() && st.mode() & 0o111 != 0 {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Replace this process with `argv`. Only returns on failure.
    fn exec_argv(&self, argv: &[Vec<u8>], extra: &[(Vec<u8>, Vec<u8>)]) -> i32 {
        let Some(path) = self.find_cmd(&argv[0]) else {
            msg(&argv[0], b"not found");
            return 127;
        };
        let mut cs: Vec<Vec<u8>> = argv.iter().map(|a| [&a[..], b"\0"].concat()).collect();
        let mut ap: Vec<usize> = cs.iter_mut().map(|c| c.as_ptr() as usize).collect();
        ap.push(0);
        let env = self.envp(extra);
        let mut ep: Vec<usize> = env.iter().map(|c| c.as_ptr() as usize).collect();
        ep.push(0);
        if !self.bg {
            sys::signal(sys::SIGINT, sys::SIG_DFL);
            sys::signal(sys::SIGQUIT, sys::SIG_DFL);
        }
        let r = sys::execve(&path, ap.as_ptr(), ep.as_ptr());
        perr(&argv[0], b"", r);
        if r == -sys::ENOENT {
            127
        } else {
            126
        }
    }

    /// Apply redirections. Returns saved (fd, copy) pairs to undo them, or None on error.
    fn redirect(&mut self, redirs: &[Redir], save: bool) -> Option<Vec<(i32, i32)>> {
        let mut saved = Vec::new();
        for r in redirs {
            let target = self.expand_one(&r.target);
            let fd = match r.kind {
                RKind::Dup => parse_u64(&target).map_or(-9, |n| n as i32),
                RKind::In => sys::open_inherit(&target, sys::O_RDONLY, 0),
                RKind::Out => {
                    sys::open_inherit(&target, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644)
                }
                RKind::Append => {
                    sys::open_inherit(&target, sys::O_WRONLY | sys::O_CREAT | sys::O_APPEND, 0o644)
                }
            };
            if fd < 0 {
                perr(b"sh", &target, fd as isize);
                self.restore(saved);
                return None;
            }
            if save {
                saved.push((r.fd, sys::dup_high(r.fd, 10)));
            }
            sys::dup2(fd, r.fd);
            if r.kind != RKind::Dup && fd != r.fd {
                sys::close(fd);
            }
        }
        Some(saved)
    }

    fn restore(&self, saved: Vec<(i32, i32)>) {
        flush();
        for (fd, copy) in saved.into_iter().rev() {
            if copy >= 0 {
                sys::dup2(copy, fd);
                sys::close(copy);
            } else {
                sys::close(fd);
            }
        }
    }

    fn wait_fg(&mut self, pid: isize) -> i32 {
        let (_, st) = sys::wait4(pid, 0);
        if st & 0x7f == sys::SIGINT as i32 {
            self.intr = true;
            out(b"\n");
        }
        sys::status_code(st)
    }

    fn run(&mut self, n: &Node, forked: bool) -> i32 {
        if self.intr || self.exit_req.is_some() || self.brk > 0 || self.cont {
            return self.last;
        }
        let code = match n {
            Node::Cmd {
                assigns,
                words,
                redirs,
            } => self.run_simple(assigns, words, redirs, forked),
            Node::Seq(items) => {
                let mut c = self.last;
                for it in items {
                    c = self.run(it, false);
                    self.last = c;
                    if self.intr || self.exit_req.is_some() || self.brk > 0 || self.cont {
                        break;
                    }
                }
                c
            }
            Node::And(a, b) => {
                let c = self.run(a, false);
                self.last = c;
                if c == 0 {
                    self.run(b, forked)
                } else {
                    c
                }
            }
            Node::Or(a, b) => {
                let c = self.run(a, false);
                self.last = c;
                if c != 0 {
                    self.run(b, forked)
                } else {
                    c
                }
            }
            Node::Not(a) => (self.run(a, false) == 0) as i32,
            Node::Pipe(cmds) => self.run_pipe(cmds),
            Node::Bg(a) => {
                flush();
                let pid = sys::fork();
                if pid == 0 {
                    // Background jobs ignore Ctrl-C, as in other shells.
                    self.interactive = false;
                    self.bg = true;
                    sys::signal(sys::SIGINT, sys::SIG_IGN);
                    sys::signal(sys::SIGQUIT, sys::SIG_IGN);
                    let c = self.run(a, true);
                    flush();
                    sys::exit(c);
                }
                self.last_bg = pid;
                if self.interactive {
                    out(b"[");
                    out_num(pid as u64);
                    out(b"]\n");
                }
                0
            }
            Node::If(arms, els) => {
                for (cond, body) in arms {
                    if self.run(cond, false) == 0 {
                        return self.run(body, false);
                    }
                }
                match els {
                    Some(e) => self.run(e, false),
                    None => 0,
                }
            }
            Node::Loop(cond, body, until) => {
                let mut c = 0;
                loop {
                    let t = self.run(cond, false);
                    if self.intr || (t == 0) == *until {
                        break;
                    }
                    c = self.run(body, false);
                    self.cont = false;
                    if self.brk > 0 {
                        self.brk -= 1;
                        break;
                    }
                    if self.intr || self.exit_req.is_some() {
                        break;
                    }
                }
                c
            }
            Node::For(var, list, body) => {
                let items = match list {
                    Some(ws) => ws.iter().flat_map(|w| self.expand(w)).collect::<Vec<_>>(),
                    None => self.pos.get(1..).unwrap_or(&[]).to_vec(),
                };
                let mut c = 0;
                for it in items {
                    self.set(var, &it, false);
                    c = self.run(body, false);
                    self.cont = false;
                    if self.brk > 0 {
                        self.brk -= 1;
                        break;
                    }
                    if self.intr || self.exit_req.is_some() {
                        break;
                    }
                }
                c
            }
        };
        self.last = code;
        code
    }

    fn run_pipe(&mut self, cmds: &[Node]) -> i32 {
        flush();
        let mut pids = Vec::new();
        let mut prev_read = -1;
        for (k, c) in cmds.iter().enumerate() {
            let last = k + 1 == cmds.len();
            let mut p = [-1i32; 2];
            if !last && sys::pipe(&mut p) < 0 {
                break;
            }
            let pid = sys::fork();
            if pid == 0 {
                if prev_read >= 0 {
                    sys::dup2(prev_read, 0);
                    sys::close(prev_read);
                }
                if !last {
                    sys::close(p[0]);
                    sys::dup2(p[1], 1);
                    sys::close(p[1]);
                }
                self.interactive = false;
                let code = self.run(c, true);
                flush();
                sys::exit(code);
            }
            if prev_read >= 0 {
                sys::close(prev_read);
            }
            if !last {
                sys::close(p[1]);
                prev_read = p[0];
            }
            pids.push(pid);
        }
        let mut code = 0;
        for (k, &pid) in pids.iter().enumerate() {
            let c = self.wait_fg(pid);
            if k + 1 == pids.len() {
                code = c;
            }
        }
        code
    }

    fn run_simple(
        &mut self,
        assigns: &[(Vec<u8>, Word)],
        words: &[Word],
        redirs: &[Redir],
        forked: bool,
    ) -> i32 {
        let mut argv = Vec::new();
        for w in words {
            argv.extend(self.expand(w));
        }
        let mut vals = Vec::new();
        for (n, w) in assigns {
            let v = self.expand_one(w);
            vals.push((n.clone(), v));
        }
        if argv.is_empty() {
            for (n, v) in vals {
                self.set(&n, &v, false);
            }
            return match self.redirect(redirs, true) {
                Some(s) => {
                    self.restore(s);
                    0
                }
                None => 1,
            };
        }
        if BUILTINS.contains(&&argv[0][..]) {
            let Some(saved) = self.redirect(redirs, true) else {
                return 1;
            };
            let c = self.builtin(&argv);
            self.restore(saved);
            return c;
        }
        if forked {
            if self.redirect(redirs, false).is_none() {
                return 1;
            }
            return self.exec_argv(&argv, &vals);
        }
        flush();
        let pid = sys::fork();
        if pid == 0 {
            if self.redirect(redirs, false).is_none() {
                sys::exit(1);
            }
            sys::exit(self.exec_argv(&argv, &vals));
        }
        if pid < 0 {
            perr(b"sh", b"fork", pid);
            return 1;
        }
        self.wait_fg(pid)
    }

    fn builtin(&mut self, argv: &[Vec<u8>]) -> i32 {
        let a: Vec<&[u8]> = argv.iter().map(|v| &v[..]).collect();
        match a[0] {
            b"cd" => {
                let home = self.get(b"HOME").unwrap_or(b"/").to_vec();
                let old = self.get(b"PWD").unwrap_or(b"/").to_vec();
                let dest = match a.get(1) {
                    None => home,
                    Some(&b"-") => self.get(b"OLDPWD").unwrap_or(b"/").to_vec(),
                    Some(d) => d.to_vec(),
                };
                let r = sys::chdir(&dest);
                if r < 0 {
                    perr(b"cd", &dest, r);
                    return 1;
                }
                let mut buf = [0u8; 1024];
                let n = sys::getcwd(&mut buf);
                if n > 0 {
                    self.set(b"PWD", &buf[..n as usize - 1], true);
                }
                self.set(b"OLDPWD", &old, true);
                0
            }
            b"exit" => {
                let c = a
                    .get(1)
                    .and_then(|s| parse_u64(s))
                    .map_or(self.last, |n| n as i32);
                self.exit_req = Some(c);
                c
            }
            b"export" => {
                if a.len() == 1 {
                    for v in self.vars.iter().filter(|v| v.export) {
                        out(b"export ");
                        out(&v.name);
                        out(b"=\"");
                        out(&v.val);
                        out(b"\"\n");
                    }
                }
                for s in &a[1..] {
                    match s.iter().position(|&c| c == b'=') {
                        Some(i) => self.set(&s[..i], &s[i + 1..], true),
                        None => {
                            let v = self.get(s).unwrap_or(b"").to_vec();
                            self.set(s, &v, true);
                        }
                    }
                }
                0
            }
            b"unset" => {
                self.vars.retain(|v| !a[1..].contains(&&v.name[..]));
                0
            }
            b"set" => {
                if a.len() > 1 {
                    // set -- a b c: replace positional parameters.
                    let start = if a[1] == b"--" { 2 } else { 1 };
                    let zero = self.pos.first().cloned().unwrap_or_default();
                    self.pos = core::iter::once(zero)
                        .chain(a[start..].iter().map(|s| s.to_vec()))
                        .collect();
                    return 0;
                }
                for v in &self.vars {
                    out(&v.name);
                    out(b"=");
                    out(&v.val);
                    out(b"\n");
                }
                0
            }
            b"shift" => {
                let n = a.get(1).and_then(|s| parse_u64(s)).unwrap_or(1) as usize;
                if self.pos.len() <= n {
                    self.pos.truncate(1);
                    return 1;
                }
                self.pos.drain(1..1 + n);
                0
            }
            b"read" => {
                flush();
                let mut line = Vec::new();
                let mut c = [0u8; 1];
                let mut got = false;
                while sys::read(0, &mut c) == 1 {
                    got = true;
                    if c[0] == b'\n' {
                        break;
                    }
                    line.push(c[0]);
                }
                let names = &a[1..];
                if names.is_empty() {
                    self.set(b"REPLY", &line, false);
                } else {
                    let mut rest: &[u8] = trim(&line);
                    for (k, n) in names.iter().enumerate() {
                        if k + 1 == names.len() {
                            self.set(n, rest, false);
                        } else {
                            let e = rest
                                .iter()
                                .position(|c| c.is_ascii_whitespace())
                                .unwrap_or(rest.len());
                            self.set(n, &rest[..e], false);
                            rest = trim(&rest[e..]);
                        }
                    }
                }
                if got {
                    0
                } else {
                    1
                }
            }
            b"." | b"source" => {
                let Some(f) = a.get(1) else { return 2 };
                match read_file(f) {
                    Ok(src) => self.run_src(&src),
                    Err(e) => {
                        perr(a[0], f, e);
                        1
                    }
                }
            }
            b"exec" => {
                if a.len() < 2 {
                    return 0;
                }
                flush();
                let c = self.exec_argv(&argv[1..], &[]);
                c
            }
            b"wait" => {
                while sys::wait4(-1, 0).0 > 0 {}
                0
            }
            b"history" => {
                for (i, h) in self.history.iter().enumerate() {
                    out_pad(&num(i as u64 + 1), 4);
                    out(b"  ");
                    out(h);
                    out(b"\n");
                }
                0
            }
            b"break" => {
                self.brk = a.get(1).and_then(|s| parse_u64(s)).unwrap_or(1) as u32;
                0
            }
            b"continue" => {
                self.cont = true;
                0
            }
            b"help" => {
                out(b"teenyOS shell. Builtins:\n ");
                for b in BUILTINS {
                    out(b" ");
                    out(b);
                }
                out(b"\nCommands:\n ");
                for (n, _) in crate::APPLETS {
                    out(b" ");
                    out(n);
                }
                out(b"\nSyntax: | && || ; & > >> < 2> 2>&1 $VAR ${VAR:-x} $(cmd) $((1+2)) * ?\n");
                out(b"        if/then/elif/else/fi  while|until/do/done  for X in ...; do ...; done\n");
                0
            }
            b":" => 0,
            // These are also applets; running them in-process avoids a fork
            // per line in scripts.
            _ => {
                let c = crate::find_applet(a[0]).map_or(127, |f| f(&a));
                flush();
                c
            }
        }
    }

    pub fn run_src(&mut self, src: &[u8]) -> i32 {
        match parse(src) {
            Ok(n) => self.run(&n, false),
            Err(ParseErr::Incomplete) => {
                msg(b"sh", b"syntax error: unexpected end of input");
                2
            }
            Err(ParseErr::Syntax(m)) => {
                msg(b"sh: syntax error", m);
                2
            }
        }
    }

    // ------------------------------------------------------------ prompt

    fn prompt(&self) -> Vec<u8> {
        let ps1 = self.get(b"PS1").unwrap_or(b"\\u@\\h:\\w\\$ ").to_vec();
        let mut p = Vec::new();
        let mut i = 0;
        while i < ps1.len() {
            if ps1[i] == b'\\' && i + 1 < ps1.len() {
                i += 1;
                match ps1[i] {
                    b'u' => p.extend_from_slice(if sys::getuid() == 0 { b"root" } else { b"user" }),
                    b'h' => {
                        let mut u = [0u8; 390];
                        sys::sc(sys::SYS_UNAME, u.as_mut_ptr() as usize, 0, 0, 0, 0);
                        p.extend_from_slice(unsafe { rt::cstr(u.as_ptr().add(65)) });
                    }
                    b'w' => {
                        let cwd = self.get(b"PWD").unwrap_or(b"?");
                        let home = self.get(b"HOME").unwrap_or(b"");
                        if !home.is_empty() && home != b"/" && cwd.starts_with(home) {
                            p.push(b'~');
                            p.extend_from_slice(&cwd[home.len()..]);
                        } else {
                            p.extend_from_slice(cwd);
                        }
                    }
                    b'$' => p.push(if sys::getuid() == 0 { b'#' } else { b'$' }),
                    b'e' => p.push(27),
                    b'n' => p.push(b'\n'),
                    c => p.push(c),
                }
            } else {
                p.push(ps1[i]);
            }
            i += 1;
        }
        p
    }

    // ------------------------------------------------------------ line editor

    fn complete(&self, line: &mut Vec<u8>, pos: &mut usize, prompt: &[u8]) {
        let start = line[..*pos]
            .iter()
            .rposition(|&c| c == b' ')
            .map_or(0, |i| i + 1);
        let word = line[start..*pos].to_vec();
        let first = line[..start].iter().all(|&c| c == b' ');
        let (dir, pre) = match word.iter().rposition(|&c| c == b'/') {
            Some(i) => (word[..i + 1].to_vec(), word[i + 1..].to_vec()),
            None => (Vec::new(), word.clone()),
        };
        let mut cands: Vec<Vec<u8>> = Vec::new();
        if first && dir.is_empty() {
            for b in BUILTINS {
                cands.push(b.to_vec());
            }
            for d in self.get(b"PATH").unwrap_or(b"/bin").split(|&c| c == b':') {
                cands.extend(list_dir(d).unwrap_or_default());
            }
        } else {
            let d = if dir.is_empty() { b"." as &[u8] } else { &dir };
            cands = list_dir(d).unwrap_or_default();
        }
        let mut m: Vec<Vec<u8>> = cands
            .into_iter()
            .filter(|c| c.starts_with(&pre) && (c[0] != b'.' || pre.first() == Some(&b'.')))
            .collect();
        m.sort();
        m.dedup();
        if m.is_empty() {
            return;
        }
        let mut common = m[0].clone();
        for c in &m[1..] {
            let n = common.iter().zip(c).take_while(|(a, b)| a == b).count();
            common.truncate(n);
        }
        let mut add = common[pre.len()..].to_vec();
        if m.len() == 1 {
            let full = [&dir[..], &m[0][..]].concat();
            let is_dir =
                sys::stat(if full.is_empty() { b"." } else { &full }).map_or(false, |s| s.is_dir());
            add.push(if is_dir && !(first && dir.is_empty()) {
                b'/'
            } else {
                b' '
            });
        } else if add.is_empty() {
            out(b"\r\n");
            for c in &m {
                out(c);
                out(b"  ");
            }
            out(b"\r\n");
            out(prompt);
            out(line);
        }
        for (k, &c) in add.iter().enumerate() {
            line.insert(*pos + k, c);
        }
        *pos += add.len();
    }

    fn read_line(&mut self, prompt: &[u8]) -> Option<Vec<u8>> {
        let Some(saved) = sys::Termios::get(0) else {
            // Not a terminal: plain byte-wise read.
            let mut line = Vec::new();
            let mut c = [0u8; 1];
            loop {
                if sys::read(0, &mut c) != 1 {
                    return if line.is_empty() { None } else { Some(line) };
                }
                if c[0] == b'\n' {
                    return Some(line);
                }
                line.push(c[0]);
            }
        };
        saved.raw(false).set(0);
        let mut line: Vec<u8> = Vec::new();
        let mut pos = 0usize;
        let mut hist = self.history.len();
        let mut stash = Vec::new();
        out(prompt);
        flush();
        let result = loop {
            let mut k = [0u8; 1];
            if sys::read(0, &mut k) != 1 {
                break None;
            }
            match k[0] {
                b'\r' | b'\n' => {
                    out(b"\r\n");
                    break Some(line);
                }
                3 => {
                    out(b"^C\r\n");
                    line.clear();
                    self.last = 130;
                    break Some(line);
                }
                4 => {
                    if line.is_empty() {
                        out(b"\r\n");
                        break None;
                    }
                    if pos < line.len() {
                        line.remove(pos);
                    }
                }
                1 => pos = 0,
                5 => pos = line.len(),
                2 => pos = pos.saturating_sub(1),
                6 => pos = (pos + 1).min(line.len()),
                21 => {
                    line.drain(..pos);
                    pos = 0;
                }
                11 => line.truncate(pos),
                12 => out(b"\x1b[H\x1b[2J"),
                23 => {
                    let mut s = pos;
                    while s > 0 && line[s - 1] == b' ' {
                        s -= 1;
                    }
                    while s > 0 && line[s - 1] != b' ' {
                        s -= 1;
                    }
                    line.drain(s..pos);
                    pos = s;
                }
                8 | 127 => {
                    if pos > 0 {
                        pos -= 1;
                        line.remove(pos);
                    }
                }
                b'\t' => self.complete(&mut line, &mut pos, prompt),
                27 => {
                    let mut seq = [0u8; 2];
                    if sys::read(0, &mut seq[..1]) != 1 || (seq[0] != b'[' && seq[0] != b'O') {
                        continue;
                    }
                    if sys::read(0, &mut seq[1..]) != 1 {
                        continue;
                    }
                    let mut code = seq[1];
                    if code.is_ascii_digit() {
                        let mut t = [0u8; 1];
                        sys::read(0, &mut t); // the trailing '~'
                        code = match code {
                            b'1' | b'7' => b'H',
                            b'4' | b'8' => b'F',
                            b'3' => b'X',
                            _ => 0,
                        };
                    }
                    match code {
                        b'A' if hist > 0 => {
                            if hist == self.history.len() {
                                stash = line.clone();
                            }
                            hist -= 1;
                            line = self.history[hist].clone();
                            pos = line.len();
                        }
                        b'B' if hist < self.history.len() => {
                            hist += 1;
                            line = if hist == self.history.len() {
                                stash.clone()
                            } else {
                                self.history[hist].clone()
                            };
                            pos = line.len();
                        }
                        b'C' => pos = (pos + 1).min(line.len()),
                        b'D' => pos = pos.saturating_sub(1),
                        b'H' => pos = 0,
                        b'F' => pos = line.len(),
                        b'X' if pos < line.len() => {
                            line.remove(pos);
                        }
                        _ => {}
                    }
                }
                c if c >= 32 => {
                    line.insert(pos, c);
                    pos += 1;
                    if pos == line.len() {
                        // Typing at the end: echo the byte, skip the redraw.
                        out(&[c]);
                        flush();
                        continue;
                    }
                }
                _ => {}
            }
            out(b"\r");
            out(prompt);
            out(&line);
            out(b"\x1b[K");
            if line.len() > pos {
                out(b"\x1b[");
                out_num((line.len() - pos) as u64);
                out(b"D");
            }
            flush();
        };
        flush();
        saved.set(0);
        result
    }

    fn interactive_loop(&mut self) -> i32 {
        loop {
            // Reap finished background jobs.
            loop {
                let (pid, st) = sys::wait4(-1, sys::WNOHANG);
                if pid <= 0 {
                    break;
                }
                out(b"[");
                out_num(pid as u64);
                out(b"] done (");
                out_num(sys::status_code(st) as u64);
                out(b")\n");
            }
            let p = self.prompt();
            let Some(mut src) = self.read_line(&p) else {
                return self.last;
            };
            // Keep reading while the input is an unfinished construct.
            loop {
                match parse(&src) {
                    Err(ParseErr::Incomplete) => {
                        let Some(more) = self.read_line(b"> ") else {
                            break;
                        };
                        src.push(b'\n');
                        src.extend_from_slice(&more);
                    }
                    _ => break,
                }
            }
            if !trim(&src).is_empty() {
                if self.history.last() != Some(&src) {
                    self.history.push(src.clone());
                    if self.history.len() > 100 {
                        self.history.remove(0);
                    }
                }
                self.intr = false;
                self.brk = 0;
                self.cont = false;
                let c = self.run_src(&src);
                self.last = c;
                flush();
            }
            if let Some(c) = self.exit_req {
                return c;
            }
        }
    }
}

pub fn main(args: &[&[u8]]) -> i32 {
    let mut sh = Shell {
        vars: Vec::new(),
        pos: Vec::new(),
        last: 0,
        last_bg: 0,
        interactive: false,
        bg: false,
        intr: false,
        exit_req: None,
        brk: 0,
        cont: false,
        history: Vec::new(),
    };
    for e in rt::env() {
        if let Some(i) = e.iter().position(|&c| c == b'=') {
            sh.set(&e[..i], &e[i + 1..], true);
        }
    }
    for (k, v) in [
        (b"PATH" as &[u8], b"/bin:/sbin" as &[u8]),
        (b"HOME", b"/root"),
        (b"TERM", b"linux"),
        (b"USER", b"root"),
    ] {
        if sh.get(k).is_none() {
            sh.set(k, v, true);
        }
    }
    let mut buf = [0u8; 1024];
    let n = sys::getcwd(&mut buf);
    if n > 0 {
        sh.set(b"PWD", &buf[..n as usize - 1], true);
    }
    sys::signal(sys::SIGTSTP, sys::SIG_IGN);
    sys::signal(sys::SIGTTOU, sys::SIG_IGN);
    sys::signal(sys::SIGTTIN, sys::SIG_IGN);

    let login = args.first().map_or(false, |a| a.first() == Some(&b'-'));
    if args.len() >= 3 && args[1] == b"-c" {
        sh.pos = args[2..].iter().map(|a| a.to_vec()).collect();
        let c = sh.run_src(args[2]);
        return sh.exit_req.unwrap_or(c);
    }
    if args.len() >= 2 {
        sh.pos = args[1..].iter().map(|a| a.to_vec()).collect();
        let src = match read_file(args[1]) {
            Ok(s) => s,
            Err(e) => {
                perr(b"sh", args[1], e);
                return 127;
            }
        };
        let c = sh.run_src(&src);
        return sh.exit_req.unwrap_or(c);
    }
    sh.pos = vec![args.first().map_or(b"sh".to_vec(), |a| a.to_vec())];
    if !sys::isatty(0) {
        let src = read_fd(0);
        let c = sh.run_src(&src);
        return sh.exit_req.unwrap_or(c);
    }
    sh.interactive = true;
    sys::signal(sys::SIGINT, sys::SIG_IGN);
    sys::signal(sys::SIGQUIT, sys::SIG_IGN);
    if login {
        // /etc/profile may set HOME, so change directory only after it.
        if let Ok(src) = read_file(b"/etc/profile") {
            sh.run_src(&src);
            flush();
        }
        let home = sh.get(b"HOME").unwrap_or(b"/").to_vec();
        if sys::chdir(&home) == 0 {
            sh.set(b"PWD", &home, true);
        }
        if let Ok(src) = read_file(&join(&home, b".profile")) {
            sh.run_src(&src);
            flush();
        }
    }
    sh.interactive_loop()
}
