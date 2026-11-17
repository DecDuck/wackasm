use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use wackasm::{
    HaltError, WackasmExecutor,
    imports::{IntoWackasmCall, WackasmImports, WackasmResource, WackasmUntypedCall},
};

pub const WASI_PREVIEW1: &str = "wasi_snapshot_preview1";
pub const WASI_PREVIEW1_IMPORTS: &[&str] = &[
    "args_get",
    "args_sizes_get",
    "clock_res_get",
    "clock_time_get",
    "environ_get",
    "environ_sizes_get",
    "fd_advise",
    "fd_allocate",
    "fd_close",
    "fd_datasync",
    "fd_fdstat_get",
    "fd_fdstat_set_flags",
    "fd_fdstat_set_rights",
    "fd_filestat_get",
    "fd_filestat_set_size",
    "fd_filestat_set_times",
    "fd_pread",
    "fd_prestat_dir_name",
    "fd_prestat_get",
    "fd_pwrite",
    "fd_read",
    "fd_readdir",
    "fd_renumber",
    "fd_seek",
    "fd_sync",
    "fd_tell",
    "fd_write",
    "path_create_directory",
    "path_filestat_get",
    "path_filestat_set_times",
    "path_link",
    "path_open",
    "path_readlink",
    "path_remove_directory",
    "path_rename",
    "path_symlink",
    "path_unlink_file",
    "poll_oneoff",
    "proc_exit",
    "proc_raise",
    "sched_yield",
    "random_get",
    "sock_accept",
    "sock_recv",
    "sock_send",
    "sock_shutdown",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct WasiErrno(pub u16);

impl WasiErrno {
    pub const SUCCESS: Self = Self(0);
    pub const BADF: Self = Self(8);
    pub const EXIST: Self = Self(20);
    pub const FBIG: Self = Self(22);
    pub const ILSEQ: Self = Self(25);
    pub const INVAL: Self = Self(28);
    pub const IO: Self = Self(29);
    pub const ISDIR: Self = Self(31);
    pub const NOENT: Self = Self(44);
    pub const NOTDIR: Self = Self(54);
    pub const NOTEMPTY: Self = Self(55);
    pub const NOTSOCK: Self = Self(57);
    pub const NOTSUP: Self = Self(58);
    pub const PERM: Self = Self(63);
    pub const NOTCAPABLE: Self = Self(76);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WasiFdstat {
    pub filetype: u8,
    pub flags: u16,
    pub rights_base: u64,
    pub rights_inheriting: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WasiFilestat {
    pub dev: u64,
    pub ino: u64,
    pub filetype: u8,
    pub nlink: u64,
    pub size: u64,
    pub atim: u64,
    pub mtim: u64,
    pub ctim: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WasiSubscription {
    pub userdata: u64,
    pub event_type: u8,
    pub clock_id: u32,
    pub timeout: u64,
    pub precision: u64,
    pub flags: u16,
    pub fd: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WasiEvent {
    pub userdata: u64,
    pub error: WasiErrno,
    pub event_type: u8,
    pub nbytes: u64,
    pub flags: u16,
}

/// Host operations available to WASI guests. Implementations should treat
/// descriptors and paths as capabilities, not as unrestricted host handles.
pub trait WackasmWasiInterface {
    fn args(&self) -> Vec<String> {
        Vec::new()
    }

    fn environ(&self) -> Vec<String> {
        Vec::new()
    }

    fn fd_read(&mut self, _fd: u32, _buffer: &mut [u8]) -> Result<usize, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_write(&mut self, _fd: u32, _buffer: &[u8]) -> Result<usize, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_close(&mut self, _fd: u32) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_seek(&mut self, _fd: u32, _offset: i64, _whence: u32) -> Result<u64, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_tell(&mut self, _fd: u32) -> Result<u64, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn preopen_name(&self, _fd: u32) -> Result<Vec<u8>, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_open(
        &mut self,
        _dirfd: u32,
        _dirflags: u32,
        _path: &[u8],
        _oflags: u16,
        _rights: u64,
        _rights_inheriting: u64,
        _fdflags: u16,
    ) -> Result<u32, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn random_get(&mut self, _buffer: &mut [u8]) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn clock_time_get(&mut self, _clock_id: u32) -> Result<u64, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn sched_yield(&mut self) -> Result<(), WasiErrno> {
        Ok(())
    }

    fn clock_res_get(&mut self, _clock_id: u32) -> Result<u64, WasiErrno> {
        Ok(1)
    }

    fn fd_advise(
        &mut self,
        _fd: u32,
        _offset: u64,
        _length: u64,
        _advice: u32,
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_allocate(&mut self, _fd: u32, _offset: u64, _length: u64) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_datasync(&mut self, _fd: u32) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_fdstat_get(&self, _fd: u32) -> Result<WasiFdstat, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_fdstat_set_flags(&mut self, _fd: u32, _flags: u16) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_fdstat_set_rights(
        &mut self,
        _fd: u32,
        _rights_base: u64,
        _rights_inheriting: u64,
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_filestat_get(&self, _fd: u32) -> Result<WasiFilestat, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_filestat_set_size(&mut self, _fd: u32, _size: u64) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_filestat_set_times(
        &mut self,
        _fd: u32,
        _atim: u64,
        _mtim: u64,
        _fst_flags: u16,
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_pread(&mut self, _fd: u32, _buffer: &mut [u8], _offset: u64) -> Result<usize, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_pwrite(&mut self, _fd: u32, _buffer: &[u8], _offset: u64) -> Result<usize, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_readdir(
        &mut self,
        _fd: u32,
        _cookie: u64,
        _buffer_len: u32,
    ) -> Result<Vec<u8>, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_renumber(&mut self, _fd: u32, _to: u32) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn fd_sync(&mut self, _fd: u32) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_create_directory(&mut self, _dirfd: u32, _path: &[u8]) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_filestat_get(
        &self,
        _dirfd: u32,
        _flags: u32,
        _path: &[u8],
    ) -> Result<WasiFilestat, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_filestat_set_times(
        &mut self,
        _dirfd: u32,
        _flags: u32,
        _path: &[u8],
        _atim: u64,
        _mtim: u64,
        _fst_flags: u16,
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_link(
        &mut self,
        _old_dirfd: u32,
        _old_flags: u32,
        _old_path: &[u8],
        _new_dirfd: u32,
        _new_path: &[u8],
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_readlink(
        &self,
        _dirfd: u32,
        _path: &[u8],
        _buffer_len: u32,
    ) -> Result<Vec<u8>, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_remove_directory(&mut self, _dirfd: u32, _path: &[u8]) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_rename(
        &mut self,
        _old_dirfd: u32,
        _old_path: &[u8],
        _new_dirfd: u32,
        _new_path: &[u8],
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_symlink(
        &mut self,
        _old_path: &[u8],
        _dirfd: u32,
        _new_path: &[u8],
    ) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn path_unlink_file(&mut self, _dirfd: u32, _path: &[u8]) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn poll_oneoff(
        &mut self,
        _subscriptions: &[WasiSubscription],
    ) -> Result<Vec<WasiEvent>, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn proc_raise(&mut self, _signal: u32) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn sock_accept(&mut self, _fd: u32, _flags: u16) -> Result<u32, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn sock_recv(
        &mut self,
        _fd: u32,
        _buffer: &mut [u8],
        _flags: u16,
    ) -> Result<(usize, u16), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn sock_send(&mut self, _fd: u32, _buffer: &[u8], _flags: u16) -> Result<usize, WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }

    fn sock_shutdown(&mut self, _fd: u32, _how: u8) -> Result<(), WasiErrno> {
        Err(WasiErrno::NOTSUP)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Wasi1;

impl<Ctx, R> WackasmImports<Ctx, R> for Wasi1
where
    Ctx: WackasmWasiInterface + 'static,
    R: WackasmResource + 'static,
{
    fn get_import_func(
        &self,
        module: &str,
        name: &str,
    ) -> Option<Box<dyn WackasmUntypedCall<Ctx, R>>> {
        if module != WASI_PREVIEW1 {
            return None;
        }

        let call = match name {
            "args_get" => args_get::<Ctx>.into_call(),
            "args_sizes_get" => args_sizes_get::<Ctx>.into_call(),
            "clock_res_get" => clock_res_get::<Ctx>.into_call(),
            "fd_advise" => fd_advise::<Ctx>.into_call(),
            "fd_allocate" => fd_allocate::<Ctx>.into_call(),
            "environ_get" => environ_get::<Ctx>.into_call(),
            "environ_sizes_get" => environ_sizes_get::<Ctx>.into_call(),
            "fd_close" => fd_close::<Ctx>.into_call(),
            "fd_datasync" => fd_datasync::<Ctx>.into_call(),
            "fd_fdstat_get" => fd_fdstat_get::<Ctx>.into_call(),
            "fd_fdstat_set_flags" => fd_fdstat_set_flags::<Ctx>.into_call(),
            "fd_fdstat_set_rights" => fd_fdstat_set_rights::<Ctx>.into_call(),
            "fd_filestat_get" => fd_filestat_get::<Ctx>.into_call(),
            "fd_filestat_set_size" => fd_filestat_set_size::<Ctx>.into_call(),
            "fd_filestat_set_times" => fd_filestat_set_times::<Ctx>.into_call(),
            "fd_pread" => fd_pread::<Ctx>.into_call(),
            "fd_prestat_dir_name" => fd_prestat_dir_name::<Ctx>.into_call(),
            "fd_prestat_get" => fd_prestat_get::<Ctx>.into_call(),
            "fd_pwrite" => fd_pwrite::<Ctx>.into_call(),
            "fd_read" => fd_read::<Ctx>.into_call(),
            "fd_readdir" => fd_readdir::<Ctx>.into_call(),
            "fd_renumber" => fd_renumber::<Ctx>.into_call(),
            "fd_seek" => fd_seek::<Ctx>.into_call(),
            "fd_sync" => fd_sync::<Ctx>.into_call(),
            "fd_tell" => fd_tell::<Ctx>.into_call(),
            "fd_write" => fd_write::<Ctx>.into_call(),
            "path_create_directory" => path_create_directory::<Ctx>.into_call(),
            "path_filestat_get" => path_filestat_get::<Ctx>.into_call(),
            "path_filestat_set_times" => path_filestat_set_times::<Ctx>.into_call(),
            "path_link" => path_link::<Ctx>.into_call(),
            "path_open" => path_open::<Ctx>.into_call(),
            "path_readlink" => path_readlink::<Ctx>.into_call(),
            "path_remove_directory" => path_remove_directory::<Ctx>.into_call(),
            "path_rename" => path_rename::<Ctx>.into_call(),
            "path_symlink" => path_symlink::<Ctx>.into_call(),
            "path_unlink_file" => path_unlink_file::<Ctx>.into_call(),
            "poll_oneoff" => poll_oneoff::<Ctx>.into_call(),
            "proc_exit" => proc_exit::<Ctx>.into_call(),
            "proc_raise" => proc_raise::<Ctx>.into_call(),
            "random_get" => random_get::<Ctx>.into_call(),
            "clock_time_get" => clock_time_get::<Ctx>.into_call(),
            "sched_yield" => sched_yield::<Ctx>.into_call(),
            "sock_accept" => sock_accept::<Ctx>.into_call(),
            "sock_recv" => sock_recv::<Ctx>.into_call(),
            "sock_send" => sock_send::<Ctx>.into_call(),
            "sock_shutdown" => sock_shutdown::<Ctx>.into_call(),
            _ => return None,
        };
        Some(call)
    }
}

macro_rules! errno_adapter {
    ($name:ident, $method:ident, $($argument:ident: $type:ty),* $(,)?) => {
        fn $name<Ctx: WackasmWasiInterface>(
            ctx: &mut Ctx,
            _executor: &mut WackasmExecutor,
            $($argument: $type),*
        ) -> Result<(i32,), HaltError> {
            Ok(errno(ctx.$method($($argument),*).err().unwrap_or(WasiErrno::SUCCESS)))
        }
    };
}

fn write_fdstat(
    executor: &mut WackasmExecutor,
    address: u32,
    stat: WasiFdstat,
) -> Result<(), HaltError> {
    let mut bytes = [0; 24];
    bytes[0] = stat.filetype;
    bytes[2..4].copy_from_slice(&stat.flags.to_le_bytes());
    bytes[8..16].copy_from_slice(&stat.rights_base.to_le_bytes());
    bytes[16..24].copy_from_slice(&stat.rights_inheriting.to_le_bytes());
    executor.write_memory(0, address, &bytes)
}

fn write_filestat(
    executor: &mut WackasmExecutor,
    address: u32,
    stat: WasiFilestat,
) -> Result<(), HaltError> {
    let mut bytes = [0; 64];
    bytes[0..8].copy_from_slice(&stat.dev.to_le_bytes());
    bytes[8..16].copy_from_slice(&stat.ino.to_le_bytes());
    bytes[16] = stat.filetype;
    bytes[24..32].copy_from_slice(&stat.nlink.to_le_bytes());
    bytes[32..40].copy_from_slice(&stat.size.to_le_bytes());
    bytes[40..48].copy_from_slice(&stat.atim.to_le_bytes());
    bytes[48..56].copy_from_slice(&stat.mtim.to_le_bytes());
    bytes[56..64].copy_from_slice(&stat.ctim.to_le_bytes());
    executor.write_memory(0, address, &bytes)
}

fn read_guest_bytes(
    executor: &WackasmExecutor,
    address: u32,
    length: u32,
) -> Result<Vec<u8>, HaltError> {
    Ok(executor.memory().read_bytes(0, address, length)?.to_vec())
}

fn clock_res_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    clock_id: u32,
    resolution: u32,
) -> Result<(i32,), HaltError> {
    match ctx.clock_res_get(clock_id) {
        Ok(value) => {
            write_u64(executor, resolution, value)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn fd_fdstat_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    stat_address: u32,
) -> Result<(i32,), HaltError> {
    match ctx.fd_fdstat_get(fd) {
        Ok(stat) => {
            write_fdstat(executor, stat_address, stat)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn fd_filestat_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    stat_address: u32,
) -> Result<(i32,), HaltError> {
    match ctx.fd_filestat_get(fd) {
        Ok(stat) => {
            write_filestat(executor, stat_address, stat)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

errno_adapter!(fd_advise, fd_advise, fd: u32, offset: u64, length: u64, advice: u32);
errno_adapter!(fd_allocate, fd_allocate, fd: u32, offset: u64, length: u64);
errno_adapter!(fd_datasync, fd_datasync, fd: u32);
errno_adapter!(fd_fdstat_set_rights, fd_fdstat_set_rights, fd: u32, rights_base: u64, rights_inheriting: u64);
errno_adapter!(fd_filestat_set_size, fd_filestat_set_size, fd: u32, size: u64);
errno_adapter!(fd_renumber, fd_renumber, fd: u32, to: u32);
errno_adapter!(fd_sync, fd_sync, fd: u32);
errno_adapter!(proc_raise, proc_raise, signal: u32);

fn fd_fdstat_set_flags<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    _executor: &mut WackasmExecutor,
    fd: u32,
    flags: u32,
) -> Result<(i32,), HaltError> {
    Ok(errno(
        ctx.fd_fdstat_set_flags(fd, flags as u16)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn fd_filestat_set_times<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    _executor: &mut WackasmExecutor,
    fd: u32,
    atim: u64,
    mtim: u64,
    fst_flags: u32,
) -> Result<(i32,), HaltError> {
    Ok(errno(
        ctx.fd_filestat_set_times(fd, atim, mtim, fst_flags as u16)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn fd_pread<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    iovs: u32,
    iovs_len: u32,
    offset: u64,
    nread: u32,
) -> Result<(i32,), HaltError> {
    let mut total = 0u32;
    for index in 0..iovs_len {
        let entry = iovs
            .checked_add(
                index.checked_mul(8).ok_or_else(|| {
                    HaltError::Error(anyhow::anyhow!("WASI iovec address overflow"))
                })?,
            )
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI iovec address overflow")))?;
        let raw = executor.memory().read_bytes(0, entry, 8)?;
        let address = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        let mut buffer = vec![0; length as usize];
        match ctx.fd_pread(fd, &mut buffer, offset + u64::from(total)) {
            Ok(read) if read <= buffer.len() => {
                buffer.truncate(read);
                executor.write_memory(0, address, &buffer)?;
                total = total
                    .checked_add(u32::try_from(read)?)
                    .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI byte count overflow")))?;
                if read < length as usize {
                    break;
                }
            }
            Ok(_) => return Ok(errno(WasiErrno::IO)),
            Err(error) => return Ok(errno(error)),
        }
    }
    write_u32(executor, nread, total)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn fd_pwrite<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    iovs: u32,
    iovs_len: u32,
    offset: u64,
    nwritten: u32,
) -> Result<(i32,), HaltError> {
    let mut total = 0u32;
    for index in 0..iovs_len {
        let entry = iovs
            .checked_add(
                index.checked_mul(8).ok_or_else(|| {
                    HaltError::Error(anyhow::anyhow!("WASI iovec address overflow"))
                })?,
            )
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI iovec address overflow")))?;
        let raw = executor.memory().read_bytes(0, entry, 8)?;
        let address = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        let input = executor.memory().read_bytes(0, address, length)?;
        match ctx.fd_pwrite(fd, input, offset + u64::from(total)) {
            Ok(written) if written <= length as usize => {
                total = total
                    .checked_add(u32::try_from(written)?)
                    .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI byte count overflow")))?;
                if written < length as usize {
                    break;
                }
            }
            Ok(_) => return Ok(errno(WasiErrno::IO)),
            Err(error) if total == 0 => return Ok(errno(error)),
            Err(_) => break,
        }
    }
    write_u32(executor, nwritten, total)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn fd_readdir<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    buffer: u32,
    buffer_len: u32,
    cookie: u64,
    buffer_used: u32,
) -> Result<(i32,), HaltError> {
    match ctx.fd_readdir(fd, cookie, buffer_len) {
        Ok(mut bytes) => {
            bytes.truncate(buffer_len as usize);
            write_u32(executor, buffer_used, u32::try_from(bytes.len())?)?;
            executor.write_memory(0, buffer, &bytes)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn path_filestat_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    flags: u32,
    path: u32,
    path_len: u32,
    stat_address: u32,
) -> Result<(i32,), HaltError> {
    let path = read_guest_bytes(executor, path, path_len)?;
    match ctx.path_filestat_get(fd, flags, &path) {
        Ok(stat) => {
            write_filestat(executor, stat_address, stat)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn path_filestat_set_times<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    flags: u32,
    path: u32,
    path_len: u32,
    atim: u64,
    mtim: u64,
    fst_flags: u32,
) -> Result<(i32,), HaltError> {
    let path = read_guest_bytes(executor, path, path_len)?;
    Ok(errno(
        ctx.path_filestat_set_times(fd, flags, &path, atim, mtim, fst_flags as u16)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn path_link<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    old_fd: u32,
    old_flags: u32,
    old_path: u32,
    old_path_len: u32,
    new_fd: u32,
    new_path: u32,
    new_path_len: u32,
) -> Result<(i32,), HaltError> {
    let old_path = read_guest_bytes(executor, old_path, old_path_len)?;
    let new_path = read_guest_bytes(executor, new_path, new_path_len)?;
    Ok(errno(
        ctx.path_link(old_fd, old_flags, &old_path, new_fd, &new_path)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn path_readlink<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    path: u32,
    path_len: u32,
    buffer: u32,
    buffer_len: u32,
    buffer_used: u32,
) -> Result<(i32,), HaltError> {
    let path = read_guest_bytes(executor, path, path_len)?;
    match ctx.path_readlink(fd, &path, buffer_len) {
        Ok(mut bytes) => {
            bytes.truncate(buffer_len as usize);
            write_u32(executor, buffer_used, u32::try_from(bytes.len())?)?;
            executor.write_memory(0, buffer, &bytes)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn path_rename<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    old_fd: u32,
    old_path: u32,
    old_path_len: u32,
    new_fd: u32,
    new_path: u32,
    new_path_len: u32,
) -> Result<(i32,), HaltError> {
    let old_path = read_guest_bytes(executor, old_path, old_path_len)?;
    let new_path = read_guest_bytes(executor, new_path, new_path_len)?;
    Ok(errno(
        ctx.path_rename(old_fd, &old_path, new_fd, &new_path)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn path_symlink<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    old_path: u32,
    old_path_len: u32,
    fd: u32,
    new_path: u32,
    new_path_len: u32,
) -> Result<(i32,), HaltError> {
    let old_path = read_guest_bytes(executor, old_path, old_path_len)?;
    let new_path = read_guest_bytes(executor, new_path, new_path_len)?;
    Ok(errno(
        ctx.path_symlink(&old_path, fd, &new_path)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn path_create_directory<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    path: u32,
    path_len: u32,
) -> Result<(i32,), HaltError> {
    let path = read_guest_bytes(executor, path, path_len)?;
    Ok(errno(
        ctx.path_create_directory(fd, &path)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn path_remove_directory<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    path: u32,
    path_len: u32,
) -> Result<(i32,), HaltError> {
    let path = read_guest_bytes(executor, path, path_len)?;
    Ok(errno(
        ctx.path_remove_directory(fd, &path)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn path_unlink_file<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    path: u32,
    path_len: u32,
) -> Result<(i32,), HaltError> {
    let path = read_guest_bytes(executor, path, path_len)?;
    Ok(errno(
        ctx.path_unlink_file(fd, &path)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn poll_oneoff<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    subscriptions: u32,
    events: u32,
    subscriptions_len: u32,
    events_len: u32,
) -> Result<(i32,), HaltError> {
    if subscriptions_len == 0 {
        return Ok(errno(WasiErrno::INVAL));
    }
    let input_len = subscriptions_len
        .checked_mul(48)
        .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI subscription buffer overflow")))?;
    let input = executor
        .memory()
        .read_bytes(0, subscriptions, input_len)?
        .to_vec();
    let mut parsed = Vec::new();
    parsed
        .try_reserve_exact(subscriptions_len as usize)
        .map_err(|error| {
            HaltError::Error(anyhow::anyhow!(
                "WASI subscription allocation failed: {error}"
            ))
        })?;
    for subscription in input.chunks_exact(48) {
        let u64_at =
            |offset| u64::from_le_bytes(subscription[offset..offset + 8].try_into().unwrap());
        parsed.push(WasiSubscription {
            userdata: u64_at(0),
            event_type: subscription[8],
            clock_id: u32::from_le_bytes(subscription[16..20].try_into().unwrap()),
            timeout: u64_at(24),
            precision: u64_at(32),
            flags: u16::from_le_bytes(subscription[40..42].try_into().unwrap()),
            fd: u32::from_le_bytes(subscription[16..20].try_into().unwrap()),
        });
    }
    match ctx.poll_oneoff(&parsed) {
        Ok(results) if results.len() <= events_len as usize => {
            for (index, event) in results.iter().enumerate() {
                let mut bytes = [0; 32];
                bytes[0..8].copy_from_slice(&event.userdata.to_le_bytes());
                bytes[8..10].copy_from_slice(&event.error.0.to_le_bytes());
                bytes[10] = event.event_type;
                bytes[16..24].copy_from_slice(&event.nbytes.to_le_bytes());
                bytes[24..26].copy_from_slice(&event.flags.to_le_bytes());
                let address = events
                    .checked_add(u32::try_from(index)?.checked_mul(32).ok_or_else(|| {
                        HaltError::Error(anyhow::anyhow!("WASI event address overflow"))
                    })?)
                    .ok_or_else(|| {
                        HaltError::Error(anyhow::anyhow!("WASI event address overflow"))
                    })?;
                executor.write_memory(0, address, &bytes)?;
            }
            write_u32(executor, events_len, u32::try_from(results.len())?)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Ok(_) => Ok(errno(WasiErrno::INVAL)),
        Err(error) => Ok(errno(error)),
    }
}

fn sock_accept<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    flags: u32,
    result_fd: u32,
) -> Result<(i32,), HaltError> {
    match ctx.sock_accept(fd, flags as u16) {
        Ok(accepted) => {
            write_u32(executor, result_fd, accepted)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn sock_recv<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    iovs: u32,
    iovs_len: u32,
    flags: u32,
    data_len: u32,
    result_flags: u32,
) -> Result<(i32,), HaltError> {
    let mut buffers = Vec::new();
    buffers
        .try_reserve_exact(iovs_len as usize)
        .map_err(|error| {
            HaltError::Error(anyhow::anyhow!("WASI iovec allocation failed: {error}"))
        })?;
    let mut capacity = 0usize;
    for index in 0..iovs_len {
        let entry = iovs
            .checked_add(
                index.checked_mul(8).ok_or_else(|| {
                    HaltError::Error(anyhow::anyhow!("WASI iovec address overflow"))
                })?,
            )
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI iovec address overflow")))?;
        let raw = executor.memory().read_bytes(0, entry, 8)?;
        let address = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        let _ = executor.memory().read_bytes(0, address, length)?;
        capacity = capacity.checked_add(length as usize).ok_or_else(|| {
            HaltError::Error(anyhow::anyhow!("WASI receive buffer length overflow"))
        })?;
        buffers.push((address, length));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(capacity).map_err(|error| {
        HaltError::Error(anyhow::anyhow!(
            "WASI receive buffer allocation failed: {error}"
        ))
    })?;
    data.resize(capacity, 0);
    match ctx.sock_recv(fd, &mut data, flags as u16) {
        Ok((received, output_flags)) if received <= capacity => {
            let mut offset = 0usize;
            let mut remaining = received;
            for (address, length) in buffers {
                let count = remaining.min(length as usize);
                executor.write_memory(0, address, &data[offset..offset + count])?;
                offset += count;
                remaining -= count;
                if remaining == 0 {
                    break;
                }
            }
            write_u32(executor, data_len, u32::try_from(received)?)?;
            executor.write_memory(0, result_flags, &output_flags.to_le_bytes())?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Ok(_) => Ok(errno(WasiErrno::IO)),
        Err(error) => Ok(errno(error)),
    }
}

fn sock_send<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    iovs: u32,
    iovs_len: u32,
    flags: u32,
    data_len: u32,
) -> Result<(i32,), HaltError> {
    let mut data = Vec::new();
    for index in 0..iovs_len {
        let entry = iovs
            .checked_add(
                index.checked_mul(8).ok_or_else(|| {
                    HaltError::Error(anyhow::anyhow!("WASI iovec address overflow"))
                })?,
            )
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI iovec address overflow")))?;
        let raw = executor.memory().read_bytes(0, entry, 8)?;
        let address = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        data.extend_from_slice(executor.memory().read_bytes(0, address, length)?);
    }
    match ctx.sock_send(fd, &data, flags as u16) {
        Ok(sent) if sent <= data.len() => {
            write_u32(executor, data_len, u32::try_from(sent)?)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Ok(_) => Ok(errno(WasiErrno::IO)),
        Err(error) => Ok(errno(error)),
    }
}

fn sock_shutdown<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    _executor: &mut WackasmExecutor,
    fd: u32,
    how: u32,
) -> Result<(i32,), HaltError> {
    Ok(errno(
        ctx.sock_shutdown(fd, how as u8)
            .err()
            .unwrap_or(WasiErrno::SUCCESS),
    ))
}

fn errno(errno: WasiErrno) -> (i32,) {
    (i32::from(errno.0),)
}

fn write_u32(executor: &mut WackasmExecutor, address: u32, value: u32) -> Result<(), HaltError> {
    executor.write_memory(0, address, &value.to_le_bytes())
}

fn write_u64(executor: &mut WackasmExecutor, address: u32, value: u64) -> Result<(), HaltError> {
    executor.write_memory(0, address, &value.to_le_bytes())
}

fn write_string_list(
    executor: &mut WackasmExecutor,
    values: &[String],
    pointers: u32,
    buffer: u32,
) -> Result<(), HaltError> {
    let mut buffer_offset = 0u32;
    for (index, value) in values.iter().enumerate() {
        let value = value.as_bytes();
        let pointer_offset = u32::try_from(index)?
            .checked_mul(4)
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI pointer array overflow")))?;
        write_u32(
            executor,
            pointers
                .checked_add(pointer_offset)
                .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI pointer array overflow")))?,
            buffer
                .checked_add(buffer_offset)
                .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI string buffer overflow")))?,
        )?;
        let mut bytes = value.to_vec();
        bytes.push(0);
        executor.write_memory(
            0,
            buffer
                .checked_add(buffer_offset)
                .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI string buffer overflow")))?,
            &bytes,
        )?;
        buffer_offset = buffer_offset
            .checked_add(u32::try_from(bytes.len())?)
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI string buffer overflow")))?;
    }
    Ok(())
}

fn string_sizes(values: &[String]) -> Result<u32, HaltError> {
    values.iter().try_fold(0u32, |size, value| {
        size.checked_add(u32::try_from(value.len())?)
            .and_then(|size| size.checked_add(1))
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI string buffer overflow")))
    })
}

fn args_sizes_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    argc: u32,
    argv_buf_size: u32,
) -> Result<(i32,), HaltError> {
    let args = ctx.args();
    write_u32(executor, argc, u32::try_from(args.len())?)?;
    write_u32(executor, argv_buf_size, string_sizes(&args)?)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn args_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    argv: u32,
    argv_buf: u32,
) -> Result<(i32,), HaltError> {
    write_string_list(executor, &ctx.args(), argv, argv_buf)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn environ_sizes_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    environ_count: u32,
    environ_buf_size: u32,
) -> Result<(i32,), HaltError> {
    let environ = ctx.environ();
    write_u32(executor, environ_count, u32::try_from(environ.len())?)?;
    write_u32(executor, environ_buf_size, string_sizes(&environ)?)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn environ_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    environ: u32,
    environ_buf: u32,
) -> Result<(i32,), HaltError> {
    write_string_list(executor, &ctx.environ(), environ, environ_buf)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn fd_close<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    _executor: &mut WackasmExecutor,
    fd: u32,
) -> Result<(i32,), HaltError> {
    Ok(errno(ctx.fd_close(fd).err().unwrap_or(WasiErrno::SUCCESS)))
}

fn fd_prestat_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    prestat: u32,
) -> Result<(i32,), HaltError> {
    match ctx.preopen_name(fd) {
        Ok(name) => {
            executor.write_memory(0, prestat, &[0])?;
            write_u32(
                executor,
                prestat
                    .checked_add(4)
                    .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI address overflow")))?,
                u32::try_from(name.len())?,
            )?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn fd_prestat_dir_name<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    path: u32,
    path_len: u32,
) -> Result<(i32,), HaltError> {
    match ctx.preopen_name(fd) {
        Ok(name) if name.len() <= path_len as usize => {
            executor.write_memory(0, path, &name)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Ok(_) => Ok(errno(WasiErrno::INVAL)),
        Err(error) => Ok(errno(error)),
    }
}

fn fd_read<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    iovs: u32,
    iovs_len: u32,
    nread: u32,
) -> Result<(i32,), HaltError> {
    let mut total = 0u32;
    for index in 0..iovs_len {
        let entry = iovs
            .checked_add(
                index.checked_mul(8).ok_or_else(|| {
                    HaltError::Error(anyhow::anyhow!("WASI iovec address overflow"))
                })?,
            )
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI iovec address overflow")))?;
        let raw = executor.memory().read_bytes(0, entry, 8)?;
        let address = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        let mut remaining = length;
        while remaining != 0 {
            let chunk_len = remaining.min(16 * 1024) as usize;
            let mut buffer = vec![0; chunk_len];
            match ctx.fd_read(fd, &mut buffer) {
                Ok(read) if read <= chunk_len => {
                    buffer.truncate(read);
                    let written_address =
                        address.checked_add(length - remaining).ok_or_else(|| {
                            HaltError::Error(anyhow::anyhow!("WASI buffer address overflow"))
                        })?;
                    executor.write_memory(0, written_address, &buffer)?;
                    total = total.checked_add(u32::try_from(read)?).ok_or_else(|| {
                        HaltError::Error(anyhow::anyhow!("WASI byte count overflow"))
                    })?;
                    remaining -= u32::try_from(read)?;
                    if read < chunk_len {
                        break;
                    }
                }
                Ok(_) => return Ok(errno(WasiErrno::IO)),
                Err(error) => return Ok(errno(error)),
            }
        }
        if remaining != 0 {
            break;
        }
    }
    write_u32(executor, nread, total)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn fd_write<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    iovs: u32,
    iovs_len: u32,
    nwritten: u32,
) -> Result<(i32,), HaltError> {
    let mut total = 0u32;
    for index in 0..iovs_len {
        let entry = iovs
            .checked_add(
                index.checked_mul(8).ok_or_else(|| {
                    HaltError::Error(anyhow::anyhow!("WASI iovec address overflow"))
                })?,
            )
            .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI iovec address overflow")))?;
        let raw = executor.memory().read_bytes(0, entry, 8)?;
        let address = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let length = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        let input = executor.memory().read_bytes(0, address, length)?;
        match ctx.fd_write(fd, input) {
            Ok(written) if written <= length as usize => {
                total = total
                    .checked_add(u32::try_from(written)?)
                    .ok_or_else(|| HaltError::Error(anyhow::anyhow!("WASI byte count overflow")))?;
                if written < length as usize {
                    break;
                }
            }
            Ok(_) => return Ok(errno(WasiErrno::IO)),
            Err(error) if total == 0 => return Ok(errno(error)),
            Err(_) => break,
        }
    }
    write_u32(executor, nwritten, total)?;
    Ok(errno(WasiErrno::SUCCESS))
}

fn fd_seek<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    offset: i64,
    whence: u32,
    new_offset: u32,
) -> Result<(i32,), HaltError> {
    match ctx.fd_seek(fd, offset, whence) {
        Ok(offset) => {
            write_u64(executor, new_offset, offset)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn fd_tell<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    fd: u32,
    offset: u32,
) -> Result<(i32,), HaltError> {
    match ctx.fd_tell(fd) {
        Ok(position) => {
            write_u64(executor, offset, position)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn path_open<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    dirfd: u32,
    dirflags: u32,
    path: u32,
    path_len: u32,
    oflags: u32,
    rights_base: u64,
    rights_inheriting: u64,
    fdflags: u32,
    opened_fd: u32,
) -> Result<(i32,), HaltError> {
    let path = executor.memory().read_bytes(0, path, path_len)?.to_vec();
    match ctx.path_open(
        dirfd,
        dirflags,
        &path,
        oflags as u16,
        rights_base,
        rights_inheriting,
        fdflags as u16,
    ) {
        Ok(fd) => {
            write_u32(executor, opened_fd, fd)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn random_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    buffer: u32,
    length: u32,
) -> Result<(i32,), HaltError> {
    let mut bytes = vec![0; length as usize];
    match ctx.random_get(&mut bytes) {
        Ok(()) => {
            executor.write_memory(0, buffer, &bytes)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn clock_time_get<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    clock_id: u32,
    _precision: u64,
    time: u32,
) -> Result<(i32,), HaltError> {
    match ctx.clock_time_get(clock_id) {
        Ok(time_value) => {
            write_u64(executor, time, time_value)?;
            Ok(errno(WasiErrno::SUCCESS))
        }
        Err(error) => Ok(errno(error)),
    }
}

fn sched_yield<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    _executor: &mut WackasmExecutor,
) -> Result<(i32,), HaltError> {
    Ok(errno(ctx.sched_yield().err().unwrap_or(WasiErrno::SUCCESS)))
}

fn proc_exit<Ctx: WackasmWasiInterface>(
    _ctx: &mut Ctx,
    _executor: &mut WackasmExecutor,
    status: u32,
) -> Result<(), HaltError> {
    Err(HaltError::Trap(anyhow::anyhow!(
        "WASI process exited with status {status}"
    )))
}

fn filetype(metadata: &std::fs::Metadata) -> u8 {
    let kind = metadata.file_type();
    if kind.is_file() {
        4
    } else if kind.is_dir() {
        3
    } else if kind.is_symlink() {
        7
    } else {
        0
    }
}

fn timestamp_ns(time: std::io::Result<SystemTime>) -> u64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos().min(u64::MAX as u128) as u64)
}

fn filestat(metadata: &std::fs::Metadata) -> WasiFilestat {
    WasiFilestat {
        filetype: filetype(metadata),
        nlink: 1,
        size: metadata.len(),
        atim: timestamp_ns(metadata.accessed()),
        mtim: timestamp_ns(metadata.modified()),
        ctim: timestamp_ns(metadata.created()),
        ..WasiFilestat::default()
    }
}

fn map_io_error(error: std::io::Error) -> WasiErrno {
    match error.kind() {
        std::io::ErrorKind::NotFound => WasiErrno::NOENT,
        std::io::ErrorKind::AlreadyExists => WasiErrno::EXIST,
        std::io::ErrorKind::PermissionDenied => WasiErrno::NOTCAPABLE,
        std::io::ErrorKind::InvalidInput => WasiErrno::INVAL,
        std::io::ErrorKind::IsADirectory => WasiErrno::ISDIR,
        std::io::ErrorKind::NotADirectory => WasiErrno::NOTDIR,
        std::io::ErrorKind::DirectoryNotEmpty => WasiErrno::NOTEMPTY,
        _ => WasiErrno::IO,
    }
}

fn set_file_times(file: &File, atim: u64, mtim: u64, flags: u16) -> Result<(), WasiErrno> {
    if flags & !0x0f != 0 {
        return Err(WasiErrno::INVAL);
    }
    let now = SystemTime::now();
    let mut times = std::fs::FileTimes::new();
    if flags & 1 != 0 || flags & 4 != 0 {
        let value = if flags & 4 != 0 {
            now
        } else {
            UNIX_EPOCH
                .checked_add(std::time::Duration::from_nanos(atim))
                .ok_or(WasiErrno::INVAL)?
        };
        times = times.set_accessed(value);
    }
    if flags & 2 != 0 || flags & 8 != 0 {
        let value = if flags & 8 != 0 {
            now
        } else {
            UNIX_EPOCH
                .checked_add(std::time::Duration::from_nanos(mtim))
                .ok_or(WasiErrno::INVAL)?
        };
        times = times.set_modified(value);
    }
    file.set_times(times).map_err(map_io_error)
}

enum StdFd {
    Stdin {
        rights: u64,
    },
    Stdout {
        rights: u64,
    },
    Stderr {
        rights: u64,
    },
    File {
        file: File,
        rights: u64,
        flags: u16,
    },
    Directory {
        path: PathBuf,
        rights: u64,
        inheriting: u64,
    },
}

/// A std-backed example host with stdin/stdout/stderr and one preopened root
/// directory at descriptor 3. Path operations are confined beneath that root.
pub struct StdWasi {
    root: PathBuf,
    args: Vec<String>,
    environ: Vec<String>,
    descriptors: HashMap<u32, StdFd>,
    next_fd: u32,
    monotonic_start: Instant,
}

impl StdWasi {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let mut descriptors = HashMap::new();
        descriptors.insert(0, StdFd::Stdin { rights: 1 << 1 });
        descriptors.insert(1, StdFd::Stdout { rights: 1 << 6 });
        descriptors.insert(2, StdFd::Stderr { rights: 1 << 6 });
        descriptors.insert(
            3,
            StdFd::Directory {
                path: root.clone(),
                rights: u64::MAX,
                inheriting: u64::MAX,
            },
        );
        Ok(Self {
            root,
            args: Vec::new(),
            environ: Vec::new(),
            descriptors,
            next_fd: 4,
            monotonic_start: Instant::now(),
        })
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args = args.into_iter().collect();
        self
    }

    pub fn with_environ(mut self, environ: impl IntoIterator<Item = String>) -> Self {
        self.environ = environ.into_iter().collect();
        self
    }

    fn checked_path_with_follow(
        &self,
        dirfd: u32,
        path: &[u8],
        follow_final: bool,
    ) -> Result<PathBuf, WasiErrno> {
        let directory = match self.descriptors.get(&dirfd) {
            Some(StdFd::Directory { path, .. }) => path,
            Some(_) => return Err(WasiErrno::NOTDIR),
            None => return Err(WasiErrno::BADF),
        };
        let path = std::str::from_utf8(path).map_err(|_| WasiErrno::ILSEQ)?;
        let relative = Path::new(path);
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(WasiErrno::NOTCAPABLE);
        }

        let candidate = directory.join(relative);
        let resolved = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() && !follow_final => {
                let parent = candidate.parent().ok_or(WasiErrno::NOENT)?;
                std::fs::canonicalize(parent)
                    .map_err(map_io_error)?
                    .join(candidate.file_name().ok_or(WasiErrno::NOENT)?)
            }
            Ok(_) => std::fs::canonicalize(&candidate).map_err(|_| WasiErrno::IO)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = candidate.parent().ok_or(WasiErrno::NOENT)?;
                std::fs::canonicalize(parent)
                    .map_err(map_io_error)?
                    .join(candidate.file_name().ok_or(WasiErrno::NOENT)?)
            }
            Err(error) => return Err(map_io_error(error)),
        };
        if !resolved.starts_with(&self.root) {
            return Err(WasiErrno::NOTCAPABLE);
        }
        Ok(resolved)
    }
}

impl WackasmWasiInterface for StdWasi {
    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn environ(&self) -> Vec<String> {
        self.environ.clone()
    }

    fn fd_read(&mut self, fd: u32, buffer: &mut [u8]) -> Result<usize, WasiErrno> {
        match self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)? {
            StdFd::Stdin { rights } if *rights & (1 << 1) != 0 => {
                std::io::stdin().read(buffer).map_err(|_| WasiErrno::IO)
            }
            StdFd::File { file, rights, .. } if *rights & (1 << 1) != 0 => {
                file.read(buffer).map_err(|_| WasiErrno::IO)
            }
            StdFd::Stdin { .. } | StdFd::File { .. } => Err(WasiErrno::NOTCAPABLE),
            _ => Err(WasiErrno::BADF),
        }
    }

    fn fd_write(&mut self, fd: u32, buffer: &[u8]) -> Result<usize, WasiErrno> {
        match self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)? {
            StdFd::Stdout { rights } if *rights & (1 << 6) != 0 => {
                std::io::stdout().write(buffer).map_err(|_| WasiErrno::IO)
            }
            StdFd::Stderr { rights } if *rights & (1 << 6) != 0 => {
                std::io::stderr().write(buffer).map_err(|_| WasiErrno::IO)
            }
            StdFd::File {
                file,
                rights,
                flags,
            } if *rights & (1 << 6) != 0 => {
                if *flags & 1 != 0 {
                    file.seek(SeekFrom::End(0)).map_err(|_| WasiErrno::IO)?;
                }
                file.write(buffer).map_err(|_| WasiErrno::IO)
            }
            StdFd::Stdout { .. } | StdFd::Stderr { .. } | StdFd::File { .. } => {
                Err(WasiErrno::NOTCAPABLE)
            }
            _ => Err(WasiErrno::BADF),
        }
    }

    fn fd_close(&mut self, fd: u32) -> Result<(), WasiErrno> {
        self.descriptors
            .remove(&fd)
            .map(|_| ())
            .ok_or(WasiErrno::BADF)
    }

    fn fd_seek(&mut self, fd: u32, offset: i64, whence: u32) -> Result<u64, WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & (1 << 2) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let seek = match whence {
            0 => SeekFrom::Start(u64::try_from(offset).map_err(|_| WasiErrno::INVAL)?),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(WasiErrno::INVAL),
        };
        file.seek(seek).map_err(|_| WasiErrno::IO)
    }

    fn fd_tell(&mut self, fd: u32) -> Result<u64, WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & (1 << 5) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        file.stream_position().map_err(|_| WasiErrno::IO)
    }

    fn fd_advise(
        &mut self,
        fd: u32,
        _offset: u64,
        _length: u64,
        advice: u32,
    ) -> Result<(), WasiErrno> {
        if !self.descriptors.contains_key(&fd) {
            return Err(WasiErrno::BADF);
        }
        if self.fd_fdstat_get(fd)?.rights_base & (1 << 7) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        if advice <= 5 {
            Ok(())
        } else {
            Err(WasiErrno::INVAL)
        }
    }

    fn fd_allocate(&mut self, fd: u32, offset: u64, length: u64) -> Result<(), WasiErrno> {
        let end = offset.checked_add(length).ok_or(WasiErrno::FBIG)?;
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & (1 << 8) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        if file.metadata().map_err(map_io_error)?.len() < end {
            file.set_len(end).map_err(map_io_error)?;
        }
        Ok(())
    }

    fn fd_datasync(&mut self, fd: u32) -> Result<(), WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & 1 == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        file.sync_data().map_err(|_| WasiErrno::IO)
    }

    fn fd_fdstat_get(&self, fd: u32) -> Result<WasiFdstat, WasiErrno> {
        let descriptor = self.descriptors.get(&fd).ok_or(WasiErrno::BADF)?;
        let (filetype, rights_base, rights_inheriting, flags) = match descriptor {
            StdFd::Stdin { rights } => (2, *rights, 0, 0),
            StdFd::Stdout { rights } | StdFd::Stderr { rights } => (2, *rights, 0, 0),
            StdFd::File { rights, flags, .. } => (4, *rights, 0, *flags),
            StdFd::Directory {
                rights, inheriting, ..
            } => (3, *rights, *inheriting, 0),
        };
        Ok(WasiFdstat {
            filetype,
            flags,
            rights_base,
            rights_inheriting,
        })
    }

    fn fd_fdstat_set_flags(&mut self, fd: u32, flags: u16) -> Result<(), WasiErrno> {
        if flags & !1 != 0 {
            return Err(WasiErrno::NOTSUP);
        }
        match self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)? {
            StdFd::File {
                flags: current,
                rights,
                ..
            } if *rights & (1 << 3) != 0 => {
                *current = flags;
                Ok(())
            }
            _ if flags == 0 => Ok(()),
            _ => Err(WasiErrno::NOTSUP),
        }
    }

    fn fd_fdstat_set_rights(
        &mut self,
        fd: u32,
        rights_base: u64,
        rights_inheriting: u64,
    ) -> Result<(), WasiErrno> {
        match self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)? {
            StdFd::Stdin { rights } | StdFd::Stdout { rights } | StdFd::Stderr { rights }
                if rights_base & !*rights == 0 && rights_inheriting == 0 =>
            {
                *rights = rights_base;
                Ok(())
            }
            StdFd::File { rights, .. } if rights_base & !*rights == 0 && rights_inheriting == 0 => {
                *rights = rights_base;
                Ok(())
            }
            StdFd::Directory {
                rights, inheriting, ..
            } if rights_base & !*rights == 0 && rights_inheriting & !*inheriting == 0 => {
                *rights = rights_base;
                *inheriting = rights_inheriting;
                Ok(())
            }
            _ => Err(WasiErrno::NOTCAPABLE),
        }
    }

    fn fd_filestat_get(&self, fd: u32) -> Result<WasiFilestat, WasiErrno> {
        let descriptor = self.descriptors.get(&fd).ok_or(WasiErrno::BADF)?;
        if self.fd_fdstat_get(fd)?.rights_base & (1 << 21) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        match descriptor {
            StdFd::File { file, .. } => file.metadata().map(|m| filestat(&m)).map_err(map_io_error),
            StdFd::Directory { path, .. } => std::fs::metadata(path)
                .map(|m| filestat(&m))
                .map_err(map_io_error),
            _ => Ok(WasiFilestat {
                filetype: 2,
                nlink: 1,
                ..WasiFilestat::default()
            }),
        }
    }

    fn fd_filestat_set_size(&mut self, fd: u32, size: u64) -> Result<(), WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & (1 << 22) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        file.set_len(size).map_err(map_io_error)
    }

    fn fd_pread(&mut self, fd: u32, buffer: &mut [u8], offset: u64) -> Result<usize, WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & ((1 << 1) | (1 << 2)) != (1 << 1) | (1 << 2) {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let previous = file.stream_position().map_err(|_| WasiErrno::IO)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| WasiErrno::IO)?;
        let read = file.read(buffer).map_err(|_| WasiErrno::IO);
        let restore = file
            .seek(SeekFrom::Start(previous))
            .map_err(|_| WasiErrno::IO);
        match (read, restore) {
            (Ok(read), Ok(_)) => Ok(read),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    fn fd_pwrite(&mut self, fd: u32, buffer: &[u8], offset: u64) -> Result<usize, WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & ((1 << 6) | (1 << 2)) != (1 << 6) | (1 << 2) {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let previous = file.stream_position().map_err(|_| WasiErrno::IO)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| WasiErrno::IO)?;
        let written = file.write(buffer).map_err(|_| WasiErrno::IO);
        let restore = file
            .seek(SeekFrom::Start(previous))
            .map_err(|_| WasiErrno::IO);
        match (written, restore) {
            (Ok(written), Ok(_)) => Ok(written),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    fn fd_readdir(&mut self, fd: u32, cookie: u64, buffer_len: u32) -> Result<Vec<u8>, WasiErrno> {
        let StdFd::Directory { path, .. } = self.descriptors.get(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::NOTDIR);
        };
        if self.fd_fdstat_get(fd)?.rights_base & (1 << 14) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let mut entries = std::fs::read_dir(path)
            .map_err(map_io_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_io_error)?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut output = Vec::new();
        for (index, entry) in entries
            .iter()
            .enumerate()
            .skip(usize::try_from(cookie).unwrap_or(usize::MAX))
        {
            let name = entry.file_name().to_string_lossy().as_bytes().to_vec();
            let total = 24usize.checked_add(name.len()).ok_or(WasiErrno::IO)?;
            if output.len().saturating_add(total) > buffer_len as usize {
                break;
            }
            let metadata = entry.metadata().map_err(map_io_error)?;
            let mut dirent = [0; 24];
            dirent[0..8].copy_from_slice(&((index + 1) as u64).to_le_bytes());
            dirent[16..20]
                .copy_from_slice(&u32::try_from(name.len()).unwrap_or(u32::MAX).to_le_bytes());
            dirent[20] = filetype(&metadata);
            output.extend_from_slice(&dirent);
            output.extend_from_slice(&name);
        }
        Ok(output)
    }

    fn fd_renumber(&mut self, fd: u32, to: u32) -> Result<(), WasiErrno> {
        let descriptor = self.descriptors.remove(&fd).ok_or(WasiErrno::BADF)?;
        self.descriptors.insert(to, descriptor);
        Ok(())
    }

    fn fd_sync(&mut self, fd: u32) -> Result<(), WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & (1 << 4) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        file.sync_all().map_err(|_| WasiErrno::IO)
    }

    fn preopen_name(&self, fd: u32) -> Result<Vec<u8>, WasiErrno> {
        if fd == 3 && matches!(self.descriptors.get(&fd), Some(StdFd::Directory { .. })) {
            Ok(b"/".to_vec())
        } else {
            Err(WasiErrno::BADF)
        }
    }

    fn clock_res_get(&mut self, clock_id: u32) -> Result<u64, WasiErrno> {
        match clock_id {
            0 | 1 => Ok(1),
            _ => Err(WasiErrno::INVAL),
        }
    }

    fn fd_filestat_set_times(
        &mut self,
        fd: u32,
        atim: u64,
        mtim: u64,
        flags: u16,
    ) -> Result<(), WasiErrno> {
        let StdFd::File { file, rights, .. } =
            self.descriptors.get_mut(&fd).ok_or(WasiErrno::BADF)?
        else {
            return Err(WasiErrno::BADF);
        };
        if *rights & (1 << 23) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        set_file_times(file, atim, mtim, flags)
    }

    fn path_create_directory(&mut self, dirfd: u32, path: &[u8]) -> Result<(), WasiErrno> {
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 9) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, false)?;
        std::fs::create_dir(path).map_err(map_io_error)
    }

    fn path_filestat_get(
        &self,
        dirfd: u32,
        flags: u32,
        path: &[u8],
    ) -> Result<WasiFilestat, WasiErrno> {
        if flags & !1 != 0 {
            return Err(WasiErrno::INVAL);
        }
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 18) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, flags & 1 != 0)?;
        let metadata = if flags & 1 != 0 {
            std::fs::metadata(path)
        } else {
            std::fs::symlink_metadata(path)
        }
        .map_err(map_io_error)?;
        Ok(filestat(&metadata))
    }

    fn path_filestat_set_times(
        &mut self,
        dirfd: u32,
        flags: u32,
        path: &[u8],
        atim: u64,
        mtim: u64,
        fst_flags: u16,
    ) -> Result<(), WasiErrno> {
        if flags & !1 != 0 {
            return Err(WasiErrno::INVAL);
        }
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 20) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, flags & 1 != 0)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(map_io_error)?;
        set_file_times(&file, atim, mtim, fst_flags)
    }

    fn path_link(
        &mut self,
        old_dirfd: u32,
        old_flags: u32,
        old_path: &[u8],
        new_dirfd: u32,
        new_path: &[u8],
    ) -> Result<(), WasiErrno> {
        if old_flags & !1 != 0 {
            return Err(WasiErrno::INVAL);
        }
        if self.fd_fdstat_get(old_dirfd)?.rights_base & (1 << 11) == 0
            || self.fd_fdstat_get(new_dirfd)?.rights_inheriting & (1 << 12) == 0
        {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let old_path = self.checked_path_with_follow(old_dirfd, old_path, old_flags & 1 != 0)?;
        let new_path = self.checked_path_with_follow(new_dirfd, new_path, false)?;
        std::fs::hard_link(old_path, new_path).map_err(map_io_error)
    }

    fn path_readlink(
        &self,
        dirfd: u32,
        path: &[u8],
        buffer_len: u32,
    ) -> Result<Vec<u8>, WasiErrno> {
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 15) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, false)?;
        let target = std::fs::read_link(path).map_err(map_io_error)?;
        let bytes = target.to_string_lossy().as_bytes().to_vec();
        Ok(bytes.into_iter().take(buffer_len as usize).collect())
    }

    fn path_remove_directory(&mut self, dirfd: u32, path: &[u8]) -> Result<(), WasiErrno> {
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 25) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, false)?;
        std::fs::remove_dir(path).map_err(map_io_error)
    }

    fn path_rename(
        &mut self,
        old_dirfd: u32,
        old_path: &[u8],
        new_dirfd: u32,
        new_path: &[u8],
    ) -> Result<(), WasiErrno> {
        if self.fd_fdstat_get(old_dirfd)?.rights_base & (1 << 16) == 0
            || self.fd_fdstat_get(new_dirfd)?.rights_inheriting & (1 << 17) == 0
        {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let old_path = self.checked_path_with_follow(old_dirfd, old_path, false)?;
        let new_path = self.checked_path_with_follow(new_dirfd, new_path, false)?;
        std::fs::rename(old_path, new_path).map_err(map_io_error)
    }

    fn path_symlink(
        &mut self,
        old_path: &[u8],
        dirfd: u32,
        new_path: &[u8],
    ) -> Result<(), WasiErrno> {
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 24) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let target = std::str::from_utf8(old_path).map_err(|_| WasiErrno::ILSEQ)?;
        if Path::new(target).is_absolute()
            || Path::new(target)
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let new_path = self.checked_path_with_follow(dirfd, new_path, false)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, new_path).map_err(map_io_error)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, new_path).map_err(map_io_error)
        }
    }

    fn path_unlink_file(&mut self, dirfd: u32, path: &[u8]) -> Result<(), WasiErrno> {
        if self.fd_fdstat_get(dirfd)?.rights_base & (1 << 26) == 0 {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, false)?;
        std::fs::remove_file(path).map_err(map_io_error)
    }

    fn path_open(
        &mut self,
        dirfd: u32,
        dirflags: u32,
        path: &[u8],
        oflags: u16,
        rights: u64,
        rights_inheriting: u64,
        fdflags: u16,
    ) -> Result<u32, WasiErrno> {
        if dirflags & !1 != 0 {
            return Err(WasiErrno::INVAL);
        }
        if fdflags & !1 != 0 {
            return Err(WasiErrno::NOTSUP);
        }
        let dir_stat = self.fd_fdstat_get(dirfd)?;
        if dir_stat.rights_base & (1 << 13) == 0
            || rights & !dir_stat.rights_inheriting != 0
            || rights_inheriting & !dir_stat.rights_inheriting != 0
        {
            return Err(WasiErrno::NOTCAPABLE);
        }
        let path = self.checked_path_with_follow(dirfd, path, dirflags & 1 != 0)?;
        if oflags & !0x0f != 0 {
            return Err(WasiErrno::INVAL);
        }
        if oflags & 2 != 0 {
            if !path.is_dir() {
                return Err(WasiErrno::NOTDIR);
            }
            let fd = self.next_fd;
            self.next_fd = self.next_fd.checked_add(1).ok_or(WasiErrno::IO)?;
            self.descriptors.insert(
                fd,
                StdFd::Directory {
                    path,
                    rights,
                    inheriting: rights_inheriting,
                },
            );
            return Ok(fd);
        }
        let read = rights & (1 << 1) != 0;
        let write = rights & (1 << 6) != 0;
        if !read && !write {
            return Err(WasiErrno::INVAL);
        }
        let mut options = OpenOptions::new();
        options
            .read(read)
            .write(write)
            .create(oflags & 1 != 0)
            .create_new(oflags & 5 == 5)
            .truncate(oflags & 8 != 0);
        let file = options.open(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => WasiErrno::NOENT,
            std::io::ErrorKind::PermissionDenied => WasiErrno::NOTCAPABLE,
            _ => WasiErrno::IO,
        })?;
        let fd = self.next_fd;
        self.next_fd = self.next_fd.checked_add(1).ok_or(WasiErrno::IO)?;
        self.descriptors.insert(
            fd,
            StdFd::File {
                file,
                rights,
                flags: fdflags,
            },
        );
        Ok(fd)
    }

    fn random_get(&mut self, buffer: &mut [u8]) -> Result<(), WasiErrno> {
        getrandom::fill(buffer).map_err(|_| WasiErrno::IO)
    }

    fn clock_time_get(&mut self, clock_id: u32) -> Result<u64, WasiErrno> {
        match clock_id {
            0 => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .map_err(|_| WasiErrno::IO),
            1 => Ok(self.monotonic_start.elapsed().as_nanos() as u64),
            _ => Err(WasiErrno::INVAL),
        }
    }

    fn poll_oneoff(
        &mut self,
        subscriptions: &[WasiSubscription],
    ) -> Result<Vec<WasiEvent>, WasiErrno> {
        if subscriptions.is_empty() {
            return Err(WasiErrno::INVAL);
        }

        let mut deadlines = Vec::with_capacity(subscriptions.len());
        let mut wait_ns = u64::MAX;
        let mut has_ready_fd = false;
        for subscription in subscriptions {
            if subscription.event_type == 0 {
                if subscription.flags & !1 != 0 {
                    deadlines.push(None);
                    continue;
                }
                let now = self.clock_time_get(subscription.clock_id)?;
                let deadline = if subscription.flags & 1 != 0 {
                    subscription.timeout
                } else {
                    now.saturating_add(subscription.timeout)
                };
                wait_ns = wait_ns.min(deadline.saturating_sub(now));
                deadlines.push(Some((subscription.clock_id, deadline)));
            } else if subscription.event_type == 1 || subscription.event_type == 2 {
                has_ready_fd |= self.descriptors.contains_key(&subscription.fd);
                deadlines.push(None);
            } else {
                deadlines.push(None);
            }
        }

        if !has_ready_fd && wait_ns != u64::MAX && wait_ns > 0 {
            std::thread::sleep(std::time::Duration::from_nanos(wait_ns));
        }

        let mut events = Vec::with_capacity(subscriptions.len());
        for (subscription, deadline) in subscriptions.iter().zip(deadlines) {
            let (error, ready) = if subscription.event_type == 0 {
                match deadline {
                    Some((clock_id, deadline)) => match self.clock_time_get(clock_id) {
                        Ok(now) => (WasiErrno::SUCCESS, now >= deadline),
                        Err(error) => (error, true),
                    },
                    None => (WasiErrno::INVAL, true),
                }
            } else if subscription.event_type == 1 || subscription.event_type == 2 {
                if self.descriptors.contains_key(&subscription.fd) {
                    (WasiErrno::SUCCESS, true)
                } else {
                    (WasiErrno::BADF, true)
                }
            } else {
                (WasiErrno::INVAL, true)
            };
            if ready {
                events.push(WasiEvent {
                    userdata: subscription.userdata,
                    error,
                    event_type: subscription.event_type,
                    nbytes: 0,
                    flags: 0,
                });
            }
        }
        Ok(events)
    }

    fn sched_yield(&mut self) -> Result<(), WasiErrno> {
        std::thread::yield_now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wackasm::imports::NoResources;

    struct TestContext;

    impl WackasmWasiInterface for TestContext {}

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wackasm-wasi-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn std_backend_supports_sandboxed_file_io() {
        let root = temporary_root();
        let mut wasi = StdWasi::new(&root).unwrap();
        let fd = wasi
            .path_open(
                3,
                0,
                b"sample.txt",
                1,
                (1 << 1) | (1 << 2) | (1 << 5) | (1 << 6),
                0,
                0,
            )
            .unwrap();
        assert_eq!(wasi.fd_write(fd, b"hello").unwrap(), 5);
        wasi.fd_seek(fd, 0, 0).unwrap();
        let mut contents = [0; 5];
        assert_eq!(wasi.fd_read(fd, &mut contents).unwrap(), 5);
        assert_eq!(&contents, b"hello");
        assert_eq!(wasi.preopen_name(3).unwrap(), b"/");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn std_backend_rejects_paths_outside_preopen() {
        let root = temporary_root();
        let mut wasi = StdWasi::new(&root).unwrap();
        assert_eq!(
            wasi.path_open(3, 0, b"../escape.txt", 1, 1 << 6, 0, 0),
            Err(WasiErrno::NOTCAPABLE)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn std_backend_does_not_inherit_host_environment() {
        let root = temporary_root();
        let wasi = StdWasi::new(&root)
            .unwrap()
            .with_args(["guest".to_owned()])
            .with_environ(["MODE=test".to_owned()]);
        assert_eq!(wasi.args(), ["guest"]);
        assert_eq!(wasi.environ(), ["MODE=test"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_table_exposes_preview1_functions() {
        let imports = Wasi1;
        for name in WASI_PREVIEW1_IMPORTS {
            assert!(
                <Wasi1 as WackasmImports<TestContext, NoResources>>::get_import_func(
                    &imports,
                    WASI_PREVIEW1,
                    name
                )
                .is_some(),
                "{name} is not registered"
            );
        }
        assert!(
            <Wasi1 as WackasmImports<TestContext, NoResources>>::get_import_func(
                &imports, "other", "fd_write"
            )
            .is_none()
        );
        assert!(
            <Wasi1 as WackasmImports<TestContext, NoResources>>::get_import_func(
                &imports,
                WASI_PREVIEW1,
                "not_a_wasi_call"
            )
            .is_none()
        );
    }
}
