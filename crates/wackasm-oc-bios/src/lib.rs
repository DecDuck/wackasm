//! The WebAssembly BIOS: a port of OpenComputers' `bios.lua` that fits on a
//! 4 KiB EEPROM.
//!
//! 1. Bind the first GPU to the first screen.
//! 2. Try the filesystem whose address is in the EEPROM's data (the boot
//!    address), then every other filesystem.
//! 3. Read `/init.wasm` into memory, beep, and `oc.exec` it. A filesystem
//!    whose image fails to load is skipped, as `bios.lua` skips a bad `init.lua`.
//! 4. Remember the filesystem that booted as the new boot address.
//! 5. Crash with "no bootable medium found" if nothing booted.
//!
//! To stay small there is no allocator and no `core::fmt`. Values are encoded
//! by hand in the format of `crates/wackasm-oc/src/value.rs`, and all slicing
//! goes through helpers that can't panic.

#![no_std]

use core::arch::wasm32;

#[link(wasm_import_module = "oc")]
unsafe extern "C" {
    fn invoke(a: *const u8, al: usize, m: *const u8, ml: usize, args: *const u8, argsl: usize) -> i32;
    fn components(f: *const u8, fl: usize) -> i32;
    fn response_len() -> i32;
    fn response_read(dst: *mut u8, cap: usize) -> i32;
    fn exec(image: *const u8, len: usize) -> i32;
    fn beep(frequency: i32, duration_ms: i32) -> i32;
    fn crash(msg: *const u8, len: usize) -> !;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    die(b"bios panicked")
}

fn die(msg: &[u8]) -> ! {
    unsafe { crash(msg.as_ptr(), msg.len()) }
}

const TAG_INT: u8 = 3;
const TAG_STRING: u8 = 5;
const TAG_BYTES: u8 = 6;
const TAG_ARRAY: u8 = 7;
const TAG_MAP: u8 = 8;
const TAG_USERDATA: u8 = 9;

/// Fixed-size byte buffer that silently stops growing when full.
struct Buf<const N: usize> {
    data: [u8; N],
    len: usize,
}

impl<const N: usize> Buf<N> {
    const fn new() -> Self {
        Self { data: [0; N], len: 0 }
    }
    fn push(&mut self, b: u8) {
        if self.len < N {
            self.data[self.len] = b;
            self.len += 1;
        }
    }
    fn extend(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.push(b);
        }
    }
    fn bytes(&self) -> &[u8] {
        &self.data[..self.len.min(N)]
    }
}

// ---------------------------------------------------------------- encoding

struct Args(Buf<256>);

impl Args {
    fn new(count: u32) -> Self {
        let mut buf = Buf::new();
        buf.push(TAG_ARRAY);
        buf.extend(&count.to_le_bytes());
        Args(buf)
    }
    fn str(mut self, s: &[u8]) -> Self {
        self.0.push(TAG_STRING);
        self.0.extend(&(s.len() as u32).to_le_bytes());
        self.0.extend(s);
        self
    }
    fn int(mut self, v: i64) -> Self {
        self.0.push(TAG_INT);
        self.0.extend(&v.to_le_bytes());
        self
    }
    fn userdata(mut self, id: u32) -> Self {
        self.0.push(TAG_USERDATA);
        self.0.extend(&id.to_le_bytes());
        self
    }
}

// ---------------------------------------------------------------- decoding

/// Big enough for a long component list or one 2 KiB `read` chunk.
static mut RESPONSE: Buf<8192> = Buf::new();

#[allow(static_mut_refs)]
fn response() -> &'static [u8] {
    unsafe {
        let n = (response_len() as usize).min(RESPONSE.data.len());
        RESPONSE.len = response_read(RESPONSE.data.as_mut_ptr(), n) as usize;
        RESPONSE.bytes()
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?]))
    }
    /// Reads an array header and returns its length.
    fn array(&mut self) -> Option<u32> {
        (self.u8()? == TAG_ARRAY).then_some(())?;
        self.u32()
    }
    /// Strings and byte strings both come back as raw bytes; `None` for nil
    /// or anything else.
    fn blob(&mut self) -> Option<&'a [u8]> {
        match self.u8()? {
            TAG_STRING | TAG_BYTES => {
                let len = self.u32()? as usize;
                self.take(len)
            }
            _ => None,
        }
    }
    fn userdata(&mut self) -> Option<u32> {
        (self.u8()? == TAG_USERDATA).then_some(())?;
        self.u32()
    }
}

// ---------------------------------------------------------------- helpers

/// Last failure, shown in the "no bootable medium found" message.
static mut REASON: Buf<128> = Buf::new();

#[allow(static_mut_refs)]
fn set_reason(msg: &[u8]) {
    unsafe {
        REASON.len = 0;
        REASON.extend(msg);
    }
}

/// Calls a component method. `Some(results)` on success (positioned after the
/// array header), or `None` with the reason recorded.
fn call(address: &[u8], method: &[u8], args: Args) -> Option<Reader<'static>> {
    let a = args.0.bytes();
    let status = unsafe {
        invoke(address.as_ptr(), address.len(), method.as_ptr(), method.len(), a.as_ptr(), a.len())
    };
    let bytes = response();
    if status != 0 {
        set_reason(bytes);
        return None;
    }
    let mut r = Reader { bytes, pos: 0 };
    r.array()?;
    Some(r)
}

/// Copies the first component of `kind` into `out`.
fn first_component(kind: &[u8], out: &mut Buf<64>) -> bool {
    out.len = 0;
    if unsafe { components(kind.as_ptr(), kind.len()) } != 0 {
        return false;
    }
    let mut r = Reader { bytes: response(), pos: 0 };
    if r.u8() != Some(TAG_MAP) || r.u32().unwrap_or(0) == 0 {
        return false;
    }
    match r.blob() {
        Some(address) => {
            out.extend(address);
            true
        }
        None => false,
    }
}

/// Where `/init.wasm` is loaded: the first page not already in use. Growing
/// memory moves this, so read it once per load.
fn image_base() -> usize {
    wasm32::memory_size(0) * 65536
}

/// Reads `/init.wasm` from `fs` into fresh memory. Returns where it landed and its length.
fn read_init(fs: &[u8]) -> Option<(usize, usize)> {
    let mut opened = call(fs, b"open", Args::new(1).str(b"/init.wasm"))?;
    let Some(handle) = opened.userdata() else {
        // open returned nil plus a reason
        set_reason(opened.blob().unwrap_or(b"cannot open /init.wasm"));
        return None;
    };

    let base = image_base();
    let mut len = 0usize;
    loop {
        let mut r = call(fs, b"read", Args::new(2).userdata(handle).int(1 << 30))?;
        let Some(chunk) = r.blob() else { break }; // nil: end of file
        let needed = base + len + chunk.len();
        let have = wasm32::memory_size(0) * 65536;
        if needed > have && wasm32::memory_grow(0, (needed - have).div_ceil(65536)) == usize::MAX {
            set_reason(b"not enough memory for /init.wasm");
            return None;
        }
        unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), (base + len) as *mut u8, chunk.len()) };
        len += chunk.len();
    }
    call(fs, b"close", Args::new(1).userdata(handle));
    Some((base, len))
}

/// Loads and runs `/init.wasm` from `fs`. Only returns if that failed.
fn try_boot(eeprom: &[u8], fs: &[u8], current: &[u8]) {
    let Some((base, len)) = read_init(fs) else { return };
    if fs != current {
        call(eeprom, b"setData", Args::new(1).str(fs));
    }
    unsafe {
        beep(1000, 200);
        exec(base as *const u8, len);
    }
    // exec failed (not wasm, wrong exports, too big...): try the next one.
    set_reason(response());
}

#[unsafe(no_mangle)]
pub extern "C" fn oc_main() {
    let mut eeprom = Buf::<64>::new();
    let mut gpu = Buf::<64>::new();
    let mut screen = Buf::<64>::new();
    if !first_component(b"eeprom", &mut eeprom) {
        die(b"no bios found; install a configured EEPROM");
    }
    if first_component(b"gpu", &mut gpu) && first_component(b"screen", &mut screen) {
        call(gpu.bytes(), b"bind", Args::new(1).str(screen.bytes()));
    }

    // The boot address lives in the EEPROM's data, as for the Lua BIOS.
    let mut boot = Buf::<64>::new();
    if let Some(mut r) = call(eeprom.bytes(), b"getData", Args::new(0)) {
        if let Some(address) = r.blob() {
            boot.extend(address);
        }
    }
    if boot.len > 0 {
        try_boot(eeprom.bytes(), boot.bytes(), boot.bytes());
    }

    // Otherwise, every filesystem in turn. Copy the list first, because
    // booting attempts overwrite the response buffer.
    let mut list = Buf::<4096>::new();
    if unsafe { components(b"filesystem".as_ptr(), 10) } == 0 {
        list.extend(response());
    }
    let mut r = Reader { bytes: list.bytes(), pos: 0 };
    if r.u8() == Some(TAG_MAP) {
        let count = r.u32().unwrap_or(0);
        for _ in 0..count {
            let (Some(address), Some(_kind)) = (r.blob(), r.blob()) else { break };
            if address != boot.bytes() {
                try_boot(eeprom.bytes(), address, boot.bytes());
            }
        }
    }

    let mut msg = Buf::<192>::new();
    msg.extend(b"no bootable medium found");
    #[allow(static_mut_refs)]
    let reason = unsafe { REASON.bytes() };
    if !reason.is_empty() {
        msg.extend(b": ");
        msg.extend(reason);
    }
    die(msg.bytes());
}

