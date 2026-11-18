//! WASI preview 1 for guest processes, over [`ProcessIo`].
//!
//! `wackasm-wasi` does the marshalling and calls the `WackasmWasiInterface`
//! methods here. Stdio is local (see [`crate::process`]). Files map onto an
//! OpenComputers filesystem component, the process's *disk*, preopened as
//! fd 3 = `/`. Java picks it on first use: the boot disk if writable, else
//! the first writable filesystem, else any (see `RUN_DEFAULT_DISK`).
//!
//! **Replay.** A filesystem component call is a Java request that suspends the
//! guest, but these trait methods must answer synchronously. So a method that
//! needs the disk asks [`HostCalls`] for the answer to its *n*-th call. If the
//! answer isn't there yet, it records the request and fails, and the
//! machine leaves the WASI import pending and hands the request to Java.
//! Java's answer is appended, and the whole import runs again from the start.
//! Earlier calls get their cached answers, until the import completes.
//!
//! That only works if a re-run is harmless, so methods change local state
//! (the fd table) only after their last disk call, and the guest memory they
//! write is the same on every run.

use serde::{Deserialize, Serialize};
use wackasm_wasi::{WackasmWasiInterface, WasiErrno, WasiFdstat, WasiFilestat};

use crate::{abi::*, machine::Request, process::ProcessIo, value::OcValue};

const STDIN: u32 = 0;
const STDOUT: u32 = 1;
const STDERR: u32 = 2;
/// The preopened root of the process's disk.
pub(crate) const ROOT_FD: u32 = 3;

const FILETYPE_CHARACTER_DEVICE: u8 = 2;
const FILETYPE_DIRECTORY: u8 = 3;
const FILETYPE_REGULAR_FILE: u8 = 4;

const OFLAGS_CREAT: u16 = 1;
const OFLAGS_DIRECTORY: u16 = 2;
const OFLAGS_EXCL: u16 = 4;
const OFLAGS_TRUNC: u16 = 8;
const RIGHTS_FD_WRITE: u64 = 1 << 6;

// errnos wackasm-wasi doesn't name.
const AGAIN: WasiErrno = WasiErrno(6);
const NODEV: WasiErrno = WasiErrno(19);
const NOSPC: WasiErrno = WasiErrno(51);
const ROFS: WasiErrno = WasiErrno(69);

/// One answered component call.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct HostAnswer {
    pub status: i32,
    pub payload: Vec<u8>,
}

/// The disk calls made by the WASI import in flight. See the module docs.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct HostCalls {
    /// Answers so far, in call order.
    pub answers: Vec<HostAnswer>,
    /// Calls answered during this run of the import.
    pub cursor: usize,
    /// The first call this run had no answer for.
    pub needed: Option<Request>,
    /// `needed` went to Java; its response belongs to this process.
    pub awaiting: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) enum Disk {
    #[default]
    Unknown,
    /// Java found no filesystem to use.
    Missing,
    Address(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum OpenFile {
    File { path: String, handle: OcValue },
    Directory { path: String },
}

impl OpenFile {
    fn path(&self) -> &str {
        match self {
            OpenFile::File { path, .. } | OpenFile::Directory { path } => path,
        }
    }
}

/// Joins a WASI path onto a directory, refusing to climb out of the disk.
pub(crate) fn resolve(base: &str, path: &[u8]) -> Result<String, WasiErrno> {
    let path = std::str::from_utf8(path).map_err(|_| WasiErrno::ILSEQ)?;
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or(WasiErrno::NOTCAPABLE)?;
            }
            part => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

/// OC reports filesystem errors as exception messages.
fn errno_for(status: i32, message: &str) -> WasiErrno {
    let message = message.to_ascii_lowercase();
    if status == STATUS_NO_SUCH_COMPONENT {
        NODEV
    } else if message.contains("read only") || message.contains("read-only") {
        ROFS
    } else if message.contains("not enough space") {
        NOSPC
    } else if message.contains("not found") || message.contains("no such file") || message.starts_with('/') {
        // FileNotFoundException's message is just the path.
        WasiErrno::NOENT
    } else {
        WasiErrno::IO
    }
}

fn first_bool(results: &[OcValue]) -> bool {
    matches!(results.first(), Some(OcValue::Bool(true)))
}

fn first_number(results: &[OcValue]) -> u64 {
    match results.first() {
        Some(OcValue::Int(n)) => (*n).max(0) as u64,
        Some(OcValue::Float(f)) => f.max(0.0) as u64,
        _ => 0,
    }
}

impl ProcessIo {
    /// The answer to this run's next disk call, or `AGAIN` after recording
    /// the request (the machine then suspends; see the module docs).
    pub(crate) fn host(&self, request: Request) -> Result<HostAnswer, WasiErrno> {
        let mut calls = self.host.borrow_mut();
        if let Some(answer) = calls.answers.get(calls.cursor).cloned() {
            calls.cursor += 1;
            return Ok(answer);
        }
        calls.needed.get_or_insert(request);
        Err(AGAIN)
    }

    pub(crate) fn disk(&self) -> Result<String, WasiErrno> {
        match &*self.disk.borrow() {
            Disk::Address(address) => return Ok(address.clone()),
            Disk::Missing => return Err(NODEV),
            Disk::Unknown => {}
        }
        // Answered straight into `self.disk` (see `OcMachine::respond`), not
        // replayed, so it is never asked twice.
        self.host.borrow_mut().needed.get_or_insert(Request::DefaultDisk);
        Err(AGAIN)
    }

    /// `filesystem.<method>(args)` on the disk.
    fn fs(&self, method: &str, args: Vec<OcValue>) -> Result<Vec<OcValue>, WasiErrno> {
        let address = self.disk()?;
        let answer = self.host(Request::Invoke {
            address,
            method: method.to_owned(),
            args,
        })?;
        if answer.status != STATUS_OK {
            return Err(errno_for(answer.status, &String::from_utf8_lossy(&answer.payload)));
        }
        match OcValue::decode(&answer.payload) {
            Ok(OcValue::Array(items)) => Ok(items),
            Ok(OcValue::Nil) | Err(_) if answer.payload.is_empty() => Ok(Vec::new()),
            _ => Err(WasiErrno::IO),
        }
    }

    fn dir_path(&self, fd: u32) -> Result<String, WasiErrno> {
        match fd {
            ROOT_FD => Ok("/".into()),
            fd => match self.files.get(&fd) {
                Some(OpenFile::Directory { path }) => Ok(path.clone()),
                Some(_) => Err(WasiErrno::NOTDIR),
                None => Err(WasiErrno::BADF),
            },
        }
    }

    fn file_handle(&self, fd: u32) -> Result<OcValue, WasiErrno> {
        match self.files.get(&fd) {
            Some(OpenFile::File { handle, .. }) => Ok(handle.clone()),
            Some(OpenFile::Directory { .. }) => Err(WasiErrno::ISDIR),
            None => Err(WasiErrno::BADF),
        }
    }

    fn stat(&self, path: &str) -> Result<WasiFilestat, WasiErrno> {
        // Two or three calls, not four: OC reports a missing file as size 0,
        // modified 0, so `exists` is only needed to tell that apart from an
        // empty file (which is rare, given a real modification time).
        let p = || vec![OcValue::str(path)];
        let dir = first_bool(&self.fs("isDirectory", p())?);
        let size = if dir { 0 } else { first_number(&self.fs("size", p())?) };
        let modified = first_number(&self.fs("lastModified", p())?);
        if !dir && size == 0 && modified == 0 && !first_bool(&self.fs("exists", p())?) {
            return Err(WasiErrno::NOENT);
        }
        // OC reports milliseconds; WASI wants nanoseconds.
        let mtim = modified.saturating_mul(1_000_000);
        Ok(WasiFilestat {
            filetype: if dir { FILETYPE_DIRECTORY } else { FILETYPE_REGULAR_FILE },
            nlink: 1,
            size,
            atim: mtim,
            mtim,
            ctim: mtim,
            ..WasiFilestat::default()
        })
    }

    fn insert(&mut self, file: OpenFile) -> u32 {
        let fd = self.next_fd.max(ROOT_FD + 1);
        self.next_fd = fd + 1;
        self.files.insert(fd, file);
        fd
    }
}

impl WackasmWasiInterface for ProcessIo {
    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn environ(&self) -> Vec<String> {
        self.env.clone()
    }

    fn fd_read(&mut self, fd: u32, buffer: &mut [u8]) -> Result<usize, WasiErrno> {
        if fd == STDIN {
            if self.stdin.is_empty() {
                // Reached only at end of file. The machine pauses the child
                // instead of calling this with an empty, still-open stdin.
                self.stdin_eof = false;
                return Ok(0);
            }
            let n = buffer.len().min(self.stdin.len());
            for (slot, byte) in buffer.iter_mut().zip(self.stdin.drain(..n)) {
                *slot = byte;
            }
            return Ok(n);
        }
        let handle = self.file_handle(fd)?;
        let chunk = self.fs("read", vec![handle, OcValue::Int(buffer.len() as i64)])?;
        let bytes = match chunk.into_iter().next() {
            Some(OcValue::Bytes(b)) => b,
            Some(OcValue::String(s)) => s.into_bytes(),
            _ => return Ok(0), // nil: end of file
        };
        let n = bytes.len().min(buffer.len());
        buffer[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    fn fd_write(&mut self, fd: u32, buffer: &[u8]) -> Result<usize, WasiErrno> {
        match fd {
            STDOUT | STDERR => {
                self.output.extend_from_slice(buffer);
                Ok(buffer.len())
            }
            fd => {
                let handle = self.file_handle(fd)?;
                self.fs("write", vec![handle, OcValue::Bytes(buffer.to_vec())])?;
                Ok(buffer.len())
            }
        }
    }

    fn fd_seek(&mut self, fd: u32, offset: i64, whence: u32) -> Result<u64, WasiErrno> {
        let whence = match whence {
            0 => "set",
            1 => "cur",
            2 => "end",
            _ => return Err(WasiErrno::INVAL),
        };
        let handle = self.file_handle(fd)?;
        Ok(first_number(&self.fs("seek", vec![handle, OcValue::str(whence), OcValue::Int(offset)])?))
    }

    fn fd_tell(&mut self, fd: u32) -> Result<u64, WasiErrno> {
        self.fd_seek(fd, 0, 1)
    }

    fn fd_close(&mut self, fd: u32) -> Result<(), WasiErrno> {
        match fd {
            STDIN | STDOUT | STDERR | ROOT_FD => Ok(()),
            fd => {
                if let OpenFile::File { handle, .. } = self.files.get(&fd).ok_or(WasiErrno::BADF)? {
                    self.fs("close", vec![handle.clone()])?;
                }
                // Last disk call done; now it's safe to change the table.
                self.files.remove(&fd);
                Ok(())
            }
        }
    }

    fn fd_sync(&mut self, _fd: u32) -> Result<(), WasiErrno> {
        Ok(())
    }

    fn fd_datasync(&mut self, _fd: u32) -> Result<(), WasiErrno> {
        Ok(())
    }

    fn fd_fdstat_get(&self, fd: u32) -> Result<WasiFdstat, WasiErrno> {
        let filetype = match fd {
            STDIN | STDOUT | STDERR => FILETYPE_CHARACTER_DEVICE,
            ROOT_FD => FILETYPE_DIRECTORY,
            fd => match self.files.get(&fd).ok_or(WasiErrno::BADF)? {
                OpenFile::File { .. } => FILETYPE_REGULAR_FILE,
                OpenFile::Directory { .. } => FILETYPE_DIRECTORY,
            },
        };
        Ok(WasiFdstat {
            filetype,
            flags: 0,
            rights_base: u64::MAX,
            rights_inheriting: u64::MAX,
        })
    }

    fn fd_filestat_get(&self, fd: u32) -> Result<WasiFilestat, WasiErrno> {
        match fd {
            STDIN | STDOUT | STDERR => Ok(WasiFilestat {
                filetype: FILETYPE_CHARACTER_DEVICE,
                nlink: 1,
                ..WasiFilestat::default()
            }),
            ROOT_FD => self.stat("/"),
            fd => self.stat(self.files.get(&fd).ok_or(WasiErrno::BADF)?.path()),
        }
    }

    fn preopen_name(&self, fd: u32) -> Result<Vec<u8>, WasiErrno> {
        if fd == ROOT_FD { Ok(b"/".to_vec()) } else { Err(WasiErrno::BADF) }
    }

    fn path_open(
        &mut self,
        dirfd: u32,
        _dirflags: u32,
        path: &[u8],
        oflags: u16,
        rights: u64,
        _rights_inheriting: u64,
        _fdflags: u16,
    ) -> Result<u32, WasiErrno> {
        let path = resolve(&self.dir_path(dirfd)?, path)?;
        let p = || vec![OcValue::str(&path)];
        let write = rights & RIGHTS_FD_WRITE != 0;

        // As few calls as Lua's io.open in the common cases: one `open`, with
        // checks only where WASI's flags need them or the open failed.
        if oflags & OFLAGS_DIRECTORY != 0 {
            if first_bool(&self.fs("isDirectory", p())?) {
                return if write { Err(WasiErrno::ISDIR) } else { Ok(self.insert(OpenFile::Directory { path })) };
            }
            return Err(if first_bool(&self.fs("exists", p())?) { WasiErrno::NOTDIR } else { WasiErrno::NOENT });
        }
        if write && oflags & (OFLAGS_EXCL | OFLAGS_CREAT) != OFLAGS_CREAT {
            // O_EXCL must fail on an existing file, and no O_CREAT on a missing one.
            let exists = first_bool(&self.fs("exists", p())?);
            if exists && oflags & OFLAGS_EXCL != 0 {
                return Err(WasiErrno::EXIST);
            }
            if !exists && oflags & OFLAGS_CREAT == 0 {
                return Err(WasiErrno::NOENT);
            }
        }
        // OC handles are read-only, truncating, or appending, so writing to an
        // existing file without O_TRUNC appends whether or not O_APPEND is set.
        let mode = if !write {
            "rb"
        } else if oflags & OFLAGS_TRUNC != 0 {
            "wb"
        } else {
            "ab" // also creates a missing file
        };
        match self.fs("open", vec![OcValue::str(&path), OcValue::str(mode)]) {
            Ok(opened) => match opened.into_iter().next() {
                Some(handle @ OcValue::Userdata(_)) => Ok(self.insert(OpenFile::File { path, handle })),
                _ => Err(WasiErrno::IO),
            },
            // OC can't open directories as files; WASI opens them for reading
            // without O_DIRECTORY too (to stat them, say).
            Err(error) if error.0 != AGAIN.0 && first_bool(&self.fs("isDirectory", p())?) => {
                if write { Err(WasiErrno::ISDIR) } else { Ok(self.insert(OpenFile::Directory { path })) }
            }
            Err(error) => Err(error),
        }
    }

    fn fd_readdir(&mut self, fd: u32, cookie: u64, buffer_len: u32) -> Result<Vec<u8>, WasiErrno> {
        let path = self.dir_path(fd)?;
        let listing = self.fs("list", vec![OcValue::str(&path)])?;
        let Some(OcValue::Array(items)) = listing.into_iter().next() else {
            return Err(WasiErrno::NOENT);
        };
        // OC marks directories with a trailing '/'.
        let mut entries: Vec<(String, u8)> = items
            .into_iter()
            .filter_map(|v| match v {
                OcValue::String(name) => Some(match name.strip_suffix('/') {
                    Some(dir) => (dir.to_owned(), FILETYPE_DIRECTORY),
                    None => (name, FILETYPE_REGULAR_FILE),
                }),
                _ => None,
            })
            .collect();
        entries.sort();

        // A full buffer means "there may be more", so fill it completely and
        // truncate the last entry; the guest asks again from its cookie.
        let mut out = Vec::new();
        for (index, (name, kind)) in entries.iter().enumerate().skip(cookie.min(usize::MAX as u64) as usize) {
            let mut dirent = [0u8; 24];
            dirent[0..8].copy_from_slice(&(index as u64 + 1).to_le_bytes()); // d_next
            dirent[8..16].copy_from_slice(&(index as u64 + 1).to_le_bytes()); // d_ino
            dirent[16..20].copy_from_slice(&(name.len() as u32).to_le_bytes());
            dirent[20] = *kind;
            out.extend_from_slice(&dirent);
            out.extend_from_slice(name.as_bytes());
            if out.len() >= buffer_len as usize {
                out.truncate(buffer_len as usize);
                break;
            }
        }
        Ok(out)
    }

    fn path_filestat_get(&self, dirfd: u32, _flags: u32, path: &[u8]) -> Result<WasiFilestat, WasiErrno> {
        self.stat(&resolve(&self.dir_path(dirfd)?, path)?)
    }

    fn path_create_directory(&mut self, dirfd: u32, path: &[u8]) -> Result<(), WasiErrno> {
        let path = resolve(&self.dir_path(dirfd)?, path)?;
        if first_bool(&self.fs("exists", vec![OcValue::str(&path)])?) {
            return Err(WasiErrno::EXIST);
        }
        if first_bool(&self.fs("makeDirectory", vec![OcValue::str(&path)])?) { Ok(()) } else { Err(WasiErrno::IO) }
    }

    fn path_unlink_file(&mut self, dirfd: u32, path: &[u8]) -> Result<(), WasiErrno> {
        let path = resolve(&self.dir_path(dirfd)?, path)?;
        let p = || vec![OcValue::str(&path)];
        if first_bool(&self.fs("isDirectory", p())?) {
            return Err(WasiErrno::ISDIR);
        }
        if first_bool(&self.fs("remove", p())?) { Ok(()) } else { Err(WasiErrno::NOENT) }
    }

    fn path_remove_directory(&mut self, dirfd: u32, path: &[u8]) -> Result<(), WasiErrno> {
        let path = resolve(&self.dir_path(dirfd)?, path)?;
        let p = || vec![OcValue::str(&path)];
        if !first_bool(&self.fs("isDirectory", p())?) {
            return Err(WasiErrno::NOTDIR);
        }
        // OC's remove is recursive; POSIX rmdir only removes empty directories.
        if let Some(OcValue::Array(items)) = self.fs("list", p())?.first() {
            if !items.is_empty() {
                return Err(WasiErrno::NOTEMPTY);
            }
        }
        if first_bool(&self.fs("remove", p())?) { Ok(()) } else { Err(WasiErrno::IO) }
    }

    fn path_rename(&mut self, old_dirfd: u32, old_path: &[u8], new_dirfd: u32, new_path: &[u8]) -> Result<(), WasiErrno> {
        let from = resolve(&self.dir_path(old_dirfd)?, old_path)?;
        let to = resolve(&self.dir_path(new_dirfd)?, new_path)?;
        if !first_bool(&self.fs("exists", vec![OcValue::str(&from)])?) {
            return Err(WasiErrno::NOENT);
        }
        if first_bool(&self.fs("rename", vec![OcValue::str(&from), OcValue::str(&to)])?) { Ok(()) } else { Err(WasiErrno::IO) }
    }

    fn random_get(&mut self, buffer: &mut [u8]) -> Result<(), WasiErrno> {
        getrandom::fill(buffer).map_err(|_| WasiErrno::IO)
    }

    /// Both clocks count machine uptime: OpenComputers has no wall clock of
    /// its own, and uptime survives saves.
    fn clock_time_get(&mut self, _clock_id: u32) -> Result<u64, WasiErrno> {
        Ok((self.uptime.max(0.0) * 1e9) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_inside_the_disk() {
        assert_eq!(resolve("/", b"a/b.txt").unwrap(), "/a/b.txt");
        assert_eq!(resolve("/a", b"./b/../c").unwrap(), "/a/c");
        assert_eq!(resolve("/", b"").unwrap(), "/");
        assert!(resolve("/", b"../etc").is_err());
        assert!(resolve("/", &[0xff]).is_err());
    }

    #[test]
    fn maps_oc_errors() {
        assert_eq!(errno_for(STATUS_ERROR, "/missing.txt").0, WasiErrno::NOENT.0);
        assert_eq!(errno_for(STATUS_ERROR, "file system is read only").0, ROFS.0);
        assert_eq!(errno_for(STATUS_NO_SUCH_COMPONENT, "no such component").0, NODEV.0);
        assert_eq!(errno_for(STATUS_ERROR, "boom").0, WasiErrno::IO.0);
    }
}
