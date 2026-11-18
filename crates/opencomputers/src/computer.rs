//! This computer: its address, uptime, memory, beeps, and power.
//! (The `computer` component itself is [`crate::components::Computer`].)

use std::time::Duration;

use crate::{component::Address, sys};

/// This computer's component address.
pub fn address() -> Address {
    let mut buf = [0u8; 64];
    let len = unsafe { sys::address(buf.as_mut_ptr(), buf.len()) }.max(0) as usize;
    Address(String::from_utf8_lossy(&buf[..len.min(buf.len())]).into_owned())
}

/// The filesystem `std::fs` uses: the disk this program sees as `/`.
/// `None` if the computer has no filesystem. On first use the architecture
/// picks the boot disk if it is writable, else the first writable disk, else
/// a read-only one; child processes share their parent's.
pub fn files_disk() -> Option<Address> {
    let status = unsafe { sys::files_disk() };
    (status == sys::STATUS_OK).then(|| Address(String::from_utf8_lossy(&sys::response()).into_owned()))
}

/// Time since the computer started, in game time. It survives world saves.
pub fn uptime() -> Duration {
    Duration::from_millis(unsafe { sys::uptime_ms() }.max(0) as u64)
}

/// Bytes of RAM this program may use (installed RAM, minus what the other
/// programs on this computer hold).
pub fn memory_limit() -> u64 {
    unsafe { sys::memory_limit() }.max(0) as u64
}

/// Beeps at `frequency` Hz (20 to 2000) for `duration`.
pub fn beep(frequency: u32, duration: Duration) {
    let ms = duration.as_millis().min(i32::MAX as u128) as i32;
    unsafe { sys::beep(frequency.min(i32::MAX as u32) as i32, ms) };
}

pub fn shutdown() -> ! {
    unsafe { sys::shutdown(0) }
}

pub fn reboot() -> ! {
    unsafe { sys::shutdown(1) }
}

/// Stops the computer with an error, shown in its GUI like a Lua crash.
pub fn crash(message: &str) -> ! {
    unsafe { sys::crash(message.as_ptr(), message.len()) }
}

/// Appends a line to this computer's server-side log (`<world>/wackasm/logs`).
pub fn log(message: &str) {
    unsafe { sys::log(message.as_ptr(), message.len()) }
}
