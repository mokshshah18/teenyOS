#![allow(dead_code)] // complete subsystem API; some entries are not yet exercised
// Per-thread process context: the file-descriptor table, current directory
// and environment. Green threads share memory but each has its own Proc, so
// the shell and applets see a normal Unix-like process.

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

/// A pipe buffer shared between a read end and a write end.
pub struct Pipe {
    pub buf: Vec<u8>,
    pub writers: u32,
    pub readers: u32,
}

#[derive(Clone)]
pub enum Fd {
    File { idx: usize, pos: usize, writable: bool, append: bool },
    Tty(u32),
    PipeRead(Rc<RefCell<Pipe>>),
    PipeWrite(Rc<RefCell<Pipe>>),
    Null,
}

pub struct Proc {
    pub fds: Vec<Option<Fd>>,
    pub cwd: Vec<u8>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub argv: Vec<Vec<u8>>,
}

impl Proc {
    pub fn new(tty: u32) -> Proc {
        let fds = vec![Some(Fd::Tty(tty)), Some(Fd::Tty(tty)), Some(Fd::Tty(tty))];
        let env = vec![
            (b"HOME".to_vec(), b"/root".to_vec()),
            (b"PATH".to_vec(), b"/bin".to_vec()),
            (b"USER".to_vec(), b"root".to_vec()),
            (b"TERM".to_vec(), b"teeny".to_vec()),
        ];
        Proc { fds, cwd: b"/root".to_vec(), env, argv: Vec::new() }
    }

    /// A child that shares the parent's fds, cwd and environment.
    pub fn fork_like(&self) -> Proc {
        Proc { fds: self.fds.clone(), cwd: self.cwd.clone(), env: self.env.clone(), argv: self.argv.clone() }
    }

    pub fn alloc_fd(&mut self, fd: Fd) -> i32 {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(fd);
                return i as i32;
            }
        }
        self.fds.push(Some(fd));
        (self.fds.len() - 1) as i32
    }

    pub fn get(&self, fd: i32) -> Option<&Fd> {
        self.fds.get(fd as usize).and_then(|s| s.as_ref())
    }

    pub fn getenv(&self, name: &[u8]) -> Option<&[u8]> {
        self.env.iter().find(|(n, _)| n == name).map(|(_, v)| &v[..])
    }

    pub fn setenv(&mut self, name: &[u8], val: &[u8]) {
        if let Some(e) = self.env.iter_mut().find(|(n, _)| n == name) {
            e.1 = val.to_vec();
        } else {
            self.env.push((name.to_vec(), val.to_vec()));
        }
    }
}
