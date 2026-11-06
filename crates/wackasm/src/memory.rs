use std::ffi::CStr;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{Data, DataKind, MemoryType, SectionLimited};

use crate::{
    HaltError, Value,
    compiler::WackasmOperator,
    const_expr::resolve_const_expr,
    functions::{FunctionManager, WackasmFunction},
    macros::{trap, unsupported},
};

pub(crate) trait MemoryNumber: Copy {
    /// The pointer must cover `size_of::<Self>()` readable bytes.
    unsafe fn read_le(pointer: *const u8) -> Self;
    /// The pointer must cover `size_of::<Self>()` writable bytes.
    unsafe fn write_le(self, pointer: *mut u8);
}

macro_rules! impl_memory_number {
    ($($number:ty),* $(,)?) => {
        $(
            impl MemoryNumber for $number {
                #[inline(always)]
                unsafe fn read_le(pointer: *const u8) -> Self {
                    Self::from_le(unsafe { pointer.cast::<Self>().read_unaligned() })
                }

                #[inline(always)]
                unsafe fn write_le(self, pointer: *mut u8) {
                    unsafe { pointer.cast::<Self>().write_unaligned(self.to_le()) };
                }
            }
        )*
    };
}

impl_memory_number!(
    u8, i8, u16, i16, u32, i32, u64, i64, u128, i128, usize, isize
);

macro_rules! impl_float_memory_number {
    ($float:ty, $bits:ty) => {
        impl MemoryNumber for $float {
            #[inline(always)]
            unsafe fn read_le(pointer: *const u8) -> Self {
                Self::from_bits(unsafe { <$bits>::read_le(pointer) })
            }

            #[inline(always)]
            unsafe fn write_le(self, pointer: *mut u8) {
                unsafe { self.to_bits().write_le(pointer) };
            }
        }
    };
}

impl_float_memory_number!(f32, u32);
impl_float_memory_number!(f64, u64);

#[cold]
#[inline(never)]
fn out_of_bounds() -> HaltError {
    trap!("out of bounds memory access")
}

#[inline(always)]
fn checked_range(start: usize, length: usize, limit: usize) -> Result<usize, HaltError> {
    match start.checked_add(length) {
        Some(end) if end <= limit => Ok(end),
        _ => Err(out_of_bounds()),
    }
}

// Dispatch-local views are never serialized and must be refreshed after memory.grow.
// `repr(C)` keeps the two words register-passable through the handler ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct MemoryView {
    pointer: *mut u8,
    len: usize,
}

impl MemoryView {
    pub(crate) const EMPTY: Self = Self {
        pointer: std::ptr::NonNull::<u8>::dangling().as_ptr(),
        len: 0,
    };

    #[inline(always)]
    fn address<T: MemoryNumber>(&self, address: u32, offset: u32) -> Option<usize> {
        // WASM32 addresses plus offsets and a fixed width fit in u64, even on 32-bit hosts.
        let start = address as u64 + offset as u64;
        let end = start + std::mem::size_of::<T>() as u64;
        if end > self.len as u64 {
            return None;
        }
        Some(start as usize)
    }

    /// Returns `None` when out of bounds, leaving the trap to the caller's cold path.
    /// The view must still refer to a live memory allocation with its current length.
    #[inline(always)]
    pub(crate) unsafe fn load<T: MemoryNumber>(&self, address: u32, offset: u32) -> Option<T> {
        let start = self.address::<T>(address, offset)?;
        Some(unsafe { T::read_le(self.pointer.add(start)) })
    }

    /// The view must still refer to a live, exclusively accessible memory allocation.
    #[inline(always)]
    pub(crate) unsafe fn store<T: MemoryNumber>(
        &self,
        address: u32,
        offset: u32,
        value: T,
    ) -> Option<()> {
        let start = self.address::<T>(address, offset)?;
        unsafe { value.write_le(self.pointer.add(start)) };
        Some(())
    }

    /// The view must still refer to a live, exclusively accessible memory allocation.
    #[inline(always)]
    pub(crate) unsafe fn fill(
        &self,
        start: usize,
        length: usize,
        value: u8,
    ) -> Result<(), HaltError> {
        checked_range(start, length, self.len)?;
        unsafe { self.pointer.add(start).write_bytes(value, length) };
        Ok(())
    }

    /// Both views must be live and exclusively accessible; ranges may overlap.
    #[inline(always)]
    pub(crate) unsafe fn copy_from(
        &self,
        source: Self,
        dest: usize,
        src: usize,
        length: usize,
    ) -> Result<(), HaltError> {
        checked_range(src, length, source.len)?;
        checked_range(dest, length, self.len)?;
        unsafe { std::ptr::copy(source.pointer.add(src), self.pointer.add(dest), length) };
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LinearMemory {
    pub(crate) buffer: Vec<u8>,
    pub(crate) page_size: usize,
    pub(crate) current: u64,
    pub(crate) max: Option<u64>,
}

impl LinearMemory {
    pub fn new(page_size: usize, current: u64, max: Option<u64>) -> Self {
        Self {
            buffer: vec![0u8; page_size * current as usize],
            page_size,
            current,
            max,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryManager {
    memories: Vec<LinearMemory>,
}

impl MemoryManager {
    pub(crate) fn new() -> Self {
        Self {
            memories: Vec::new(),
        }
    }

    pub(crate) fn memory0(&mut self) -> MemoryView {
        match self.memories.first_mut() {
            Some(memory) => MemoryView {
                pointer: memory.buffer.as_mut_ptr(),
                len: memory.buffer.len(),
            },
            None => MemoryView::EMPTY,
        }
    }

    /// `index` must name a memory validated by wasmparser for the current module.
    #[inline(always)]
    pub(crate) unsafe fn view_validated(&mut self, index: u32) -> MemoryView {
        debug_assert!((index as usize) < self.memories.len());
        let memory = unsafe { self.memories.get_unchecked_mut(index as usize) };
        MemoryView {
            pointer: memory.buffer.as_mut_ptr(),
            len: memory.buffer.len(),
        }
    }

    pub(crate) fn validate_compiled_indexes(
        &self,
        functions: &FunctionManager,
    ) -> Result<(), HaltError> {
        let validate = |index: u32| -> Result<(), HaltError> {
            if index as usize >= self.memories.len() {
                Err(anyhow!("invalid compiled memory index {}", index).into())
            } else {
                Ok(())
            }
        };
        use WackasmOperator::*;
        for function in &functions.function_bodies {
            let WackasmFunction::Internal(body) = function else {
                continue;
            };
            for operator in &body.runtime {
                match &operator.op() {
                    I32Load { memarg, .. }
                    | I32Load8U { memarg, .. }
                    | I32Load8S { memarg, .. }
                    | I32Load16U { memarg, .. }
                    | I32Load16S { memarg, .. }
                    | I64Load { memarg, .. }
                    | I64Load8U { memarg, .. }
                    | I64Load8S { memarg, .. }
                    | I64Load16U { memarg, .. }
                    | I64Load16S { memarg, .. }
                    | I64Load32U { memarg, .. }
                    | I64Load32S { memarg, .. }
                    | F32Load { memarg, .. }
                    | F64Load { memarg, .. }
                    | V128Load { memarg, .. }
                    | I32Store { memarg, .. }
                    | I32Store8 { memarg, .. }
                    | I32Store16 { memarg, .. }
                    | I64Store { memarg, .. }
                    | I64Store8 { memarg, .. }
                    | I64Store16 { memarg, .. }
                    | I64Store32 { memarg, .. }
                    | F32Store { memarg, .. }
                    | F64Store { memarg, .. }
                    | V128Store { memarg, .. }
                    | I32LoadAt { memarg, .. }
                    | I32Load8UAt { memarg, .. }
                    | I32Load8SAt { memarg, .. }
                    | I32Load16UAt { memarg, .. }
                    | I32Load16SAt { memarg, .. }
                    | I64LoadAt { memarg, .. }
                    | I64Load8UAt { memarg, .. }
                    | I64Load8SAt { memarg, .. }
                    | I64Load16UAt { memarg, .. }
                    | I64Load16SAt { memarg, .. }
                    | I64Load32UAt { memarg, .. }
                    | I64Load32SAt { memarg, .. }
                    | F32LoadAt { memarg, .. }
                    | F64LoadAt { memarg, .. }
                    | V128LoadAt { memarg, .. }
                    | I32StoreAt { memarg, .. }
                    | I32Store8At { memarg, .. }
                    | I32Store16At { memarg, .. }
                    | I64StoreAt { memarg, .. }
                    | I64Store8At { memarg, .. }
                    | I64Store16At { memarg, .. }
                    | I64Store32At { memarg, .. }
                    | F32StoreAt { memarg, .. }
                    | F64StoreAt { memarg, .. }
                    | V128StoreAt { memarg, .. } => validate(memarg.memory)?,
                    MemoryGrow { mem, .. } | MemoryFill { mem, .. } => validate(*mem)?,
                    MemoryCopy {
                        src_mem, dst_mem, ..
                    } => {
                        validate(*src_mem as u32)?;
                        validate(*dst_mem as u32)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(crate) fn process_memory_section(
        &mut self,
        raw_memories: SectionLimited<MemoryType>,
    ) -> Result<(), HaltError> {
        for memory in raw_memories {
            let memory = memory?;
            self.memories.push(LinearMemory::new(
                memory.page_size() as usize,
                memory.initial,
                memory.maximum,
            ));
        }

        Ok(())
    }

    pub(crate) fn process_data_section(
        &mut self,
        raw_data: SectionLimited<Data>,
    ) -> Result<(), HaltError> {
        for data in raw_data {
            let data = data?;
            match data.kind {
                DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    let offset_value = resolve_const_expr(offset_expr)?
                        .into_value_checked()
                        .ok_or_else(|| {
                            anyhow!("values larger than a u64 not supported for data section")
                        })?;

                    let linear_memory = self
                        .memories
                        .get_mut(memory_index as usize)
                        .ok_or_else(|| anyhow!("no linear memory for index {}", memory_index))?;

                    let start = usize::try_from(offset_value.0 as i32 as u32)?;
                    let end = start
                        .checked_add(data.data.len())
                        .ok_or_else(|| trap!("out of bounds memory access"))?;

                    let destination = linear_memory
                        .buffer
                        .get_mut(start..end)
                        .ok_or_else(|| trap!("out of bounds memory access"))?;

                    destination.copy_from_slice(data.data);
                }
                DataKind::Passive => {
                    return Err(unsupported!("passive data segments are not supported"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn grow(&mut self, mem: u32, amount: i32) -> Result<i32, HaltError> {
        let memory = self
            .memories
            .get_mut(mem as usize)
            .ok_or_else(|| anyhow!("memory not found for index {}", mem))?;

        let prev = memory.current;

        // No more than 4GiB of pages
        let hard_max_pages = (1u64 << 32) / memory.page_size as u64;

        let limit = memory
            .max
            .map_or(hard_max_pages, |max| max.min(hard_max_pages));
        let Some(new) = prev
            .checked_add(amount as u32 as u64)
            .filter(|new| *new <= limit)
        else {
            return Ok(-1);
        };

        let new_len = new as usize * memory.page_size;
        if memory
            .buffer
            .try_reserve_exact(new_len - memory.buffer.len())
            .is_err()
        {
            return Ok(-1);
        }
        memory.buffer.resize(new_len, 0);
        memory.current = new;

        Ok(prev.try_into()?)
    }
}

impl MemoryManager {
    /// Raw view of a linear memory. Index 0 is the default memory.
    pub fn bytes(&self, mem: u32) -> Option<&[u8]> {
        self.memories.get(mem as usize).map(|m| m.buffer.as_slice())
    }

    pub fn read_bytes(&self, mem: u32, addr: u32, len: u32) -> Result<&[u8], HaltError> {
        let start = addr as usize;
        let bytes = self
            .bytes(mem)
            .ok_or_else(|| anyhow!("no linear memory for index {}", mem))?;
        let end = checked_range(start, len as usize, bytes.len())?;
        Ok(unsafe { bytes.get_unchecked(start..end) })
    }

    /// Reads a NUL-terminated string starting at `addr`.
    pub fn read_cstr(&self, mem: u32, addr: u32) -> Result<&CStr, HaltError> {
        let tail = self
            .bytes(mem)
            .ok_or_else(|| anyhow!("no linear memory for index {}", mem))?
            .get(addr as usize..)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        CStr::from_bytes_until_nul(tail).map_err(|_| trap!("unterminated string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: MemoryNumber + PartialEq + std::fmt::Debug>(value: T, expected: &[u8]) {
        let mut bytes = vec![0; expected.len() + 2];
        let view = MemoryView {
            pointer: bytes.as_mut_ptr(),
            len: bytes.len(),
        };
        unsafe { view.store(1, 0, value).unwrap() };
        assert_eq!(&bytes[1..1 + expected.len()], expected);
        assert_eq!(unsafe { view.load::<T>(0, 1).unwrap() }, value);
        assert!(unsafe { view.load::<T>(3, 0) }.is_none());
        assert!(unsafe { view.store(3, 0, value) }.is_none());
        assert_eq!(&bytes[1..1 + expected.len()], expected);
        assert!(unsafe { view.load::<T>(u32::MAX, u32::MAX) }.is_none());
        assert!(unsafe { view.store(u32::MAX, u32::MAX, value) }.is_none());
    }

    #[test]
    fn unaligned_accesses_are_little_endian_and_bounds_checked_for_every_width() {
        roundtrip(0x81u8, &[0x81]);
        roundtrip(-127i8, &[0x81]);
        roundtrip(0x8182u16, &[0x82, 0x81]);
        roundtrip(-32382i16, &[0x82, 0x81]);
        roundtrip(0x81828384u32, &[0x84, 0x83, 0x82, 0x81]);
        roundtrip(0x81828384u32 as i32, &[0x84, 0x83, 0x82, 0x81]);
        roundtrip(
            0x8182838485868788u64,
            &[0x88, 0x87, 0x86, 0x85, 0x84, 0x83, 0x82, 0x81],
        );
        roundtrip(
            0x8182838485868788u64 as i64,
            &[0x88, 0x87, 0x86, 0x85, 0x84, 0x83, 0x82, 0x81],
        );
        let wide = 0x8182838485868788898a8b8c8d8e8f90u128;
        roundtrip(wide, &wide.to_le_bytes());
        roundtrip(wide as i128, &wide.to_le_bytes());
        roundtrip(-1.5f32, &(-1.5f32).to_le_bytes());
        roundtrip(-1.5f64, &(-1.5f64).to_le_bytes());
    }

    #[test]
    fn floating_point_nan_payloads_are_preserved() {
        let mut bytes = [0; 16];
        let view = MemoryView {
            pointer: bytes.as_mut_ptr(),
            len: bytes.len(),
        };
        let f32_bits = 0xff812345;
        let f64_bits = 0xfff0123456789abc;
        unsafe {
            view.store(1, 0, f32::from_bits(f32_bits)).unwrap();
            view.store(5, 0, f64::from_bits(f64_bits)).unwrap();
            assert_eq!(view.load::<f32>(1, 0).unwrap().to_bits(), f32_bits);
            assert_eq!(view.load::<f64>(5, 0).unwrap().to_bits(), f64_bits);
        }
    }

    #[test]
    fn bulk_ranges_are_checked_before_writes_and_allow_overlap() {
        let mut bytes = [0, 1, 2, 3, 4, 5, 6, 7];
        let view = MemoryView {
            pointer: bytes.as_mut_ptr(),
            len: bytes.len(),
        };
        unsafe {
            view.copy_from(view, 2, 0, 6).unwrap();
            assert_eq!(bytes, [0, 1, 0, 1, 2, 3, 4, 5]);
            let before = bytes;
            assert!(view.copy_from(view, 0, 4, 5).is_err());
            assert_eq!(bytes, before);
            assert!(view.copy_from(view, 4, 0, 5).is_err());
            assert_eq!(bytes, before);
            assert!(view.copy_from(view, usize::MAX, 0, 2).is_err());
            assert!(view.fill(usize::MAX, 2, 0).is_err());
            assert!(view.fill(7, 2, 0).is_err());
            assert_eq!(bytes, before);
            view.fill(8, 0, 0).unwrap();
            view.copy_from(view, 8, 8, 0).unwrap();
            assert!(view.fill(9, 0, 0).is_err());
            assert!(view.copy_from(view, 8, 9, 0).is_err());
            assert!(view.copy_from(view, 9, 8, 0).is_err());
        }
    }
}
