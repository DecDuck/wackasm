use std::{collections::HashMap, ffi::CStr};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{Data, DataKind, MemArg, MemoryType, SectionLimited};

use crate::{
    HaltError, Value,
    compiler::WackasmMemArg,
    const_expr::resolve_const_expr,
    macros::{trap, unsupported},
};

pub(crate) trait MemoryNumber: Copy {
    const SIZE: usize;

    fn from_le_slice(bytes: &[u8]) -> Result<Self, HaltError>;
    fn write_le_slice(self, bytes: &mut [u8]);
}

macro_rules! impl_memory_number {
    ($($number:ty),* $(,)?) => {
        $(
            impl MemoryNumber for $number {
                const SIZE: usize = ::core::mem::size_of::<$number>();

                fn from_le_slice(bytes: &[u8]) -> Result<Self, HaltError> {
                    let bytes: [u8; ::core::mem::size_of::<$number>()] = bytes
                        .try_into()
                        .map_err(|_| anyhow!("invalid byte width for {}", stringify!($number)))?;
                    Ok(<$number>::from_le_bytes(bytes))
                }

                fn write_le_slice(self, bytes: &mut [u8]) {
                    bytes.copy_from_slice(&self.to_le_bytes());
                }
            }
        )*
    };
}

impl_memory_number!(
    u8, i8, u16, i16, u32, i32, u64, i64, u128, i128, usize, isize, f32, f64
);

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

    pub(crate) fn fill(
        &mut self,
        mem: u32,
        start: usize,
        length: usize,
        value: u8,
    ) -> Result<(), HaltError> {
        let memory = self
            .memories
            .get_mut(mem as usize)
            .ok_or_else(|| anyhow!("memory not found for index {}", mem))?;

        let end = start
            .checked_add(length)
            .ok_or_else(|| trap!("out of bounds memory access"))?;

        let destination = memory
            .buffer
            .get_mut(start..end)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        destination.fill(value);

        Ok(())
    }

    pub(crate) fn load<T: MemoryNumber>(
        &self,
        memarg: &WackasmMemArg,
        address: u32,
    ) -> Result<T, HaltError> {
        let memory = self
            .memories
            .get(memarg.memory as usize)
            .ok_or_else(|| anyhow!("no linear memory for index {}", memarg.memory))?;

        let start = usize::try_from(address)?
            .checked_add(usize::try_from(memarg.offset)?)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        let end = start
            .checked_add(T::SIZE)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        let bytes = memory
            .buffer
            .get(start..end)
            .ok_or_else(|| trap!("out of bounds memory access"))?;

        T::from_le_slice(bytes)
    }

    pub(crate) fn store<T: MemoryNumber>(
        &mut self,
        memarg: &WackasmMemArg,
        address: u32,
        value: T,
    ) -> Result<(), HaltError> {
        let memory = self
            .memories
            .get_mut(memarg.memory as usize)
            .ok_or_else(|| anyhow!("no linear memory for index {}", memarg.memory))?;

        let start = usize::try_from(address)?
            .checked_add(usize::try_from(memarg.offset)?)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        let end = start
            .checked_add(T::SIZE)
            .ok_or_else(|| trap!("out of bounds memory access"))?;

        let destination = memory
            .buffer
            .get_mut(start..end)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        value.write_le_slice(destination);

        Ok(())
    }

    pub(crate) fn memcpy(
        &mut self,
        source: u32,
        target: u32,
        dest: usize,
        src: usize,
        length: usize,
    ) -> Result<(), HaltError> {
        let src_end = src
            .checked_add(length)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        let dest_end = dest
            .checked_add(length)
            .ok_or_else(|| trap!("out of bounds memory access"))?;

        if source != target {
            let [src_mem, dest_mem] = self
                .memories
                .get_disjoint_mut([source as usize, target as usize])
                .map_err(|_| trap!("out of bounds memory access"))?;

            let source_range = src_mem
                .buffer
                .get(src..src_end)
                .ok_or_else(|| trap!("out of bounds memory access"))?;
            let target_range = dest_mem
                .buffer
                .get_mut(dest..dest_end)
                .ok_or_else(|| trap!("out of bounds memory access"))?;
            target_range.copy_from_slice(source_range);
        } else {
            let memory = self
                .memories
                .get_mut(source as usize)
                .ok_or_else(|| anyhow!("no linear memory for index {}", source))?;

            if src_end > memory.buffer.len() || dest_end > memory.buffer.len() {
                return Err(trap!("out of bounds memory access"));
            }
            memory.buffer.copy_within(src..src_end, dest);
        }

        Ok(())
    }
}

impl MemoryManager {
    /// Raw view of a linear memory. Index 0 is the default memory.
    pub fn bytes(&self, mem: u32) -> Option<&[u8]> {
        self.memories.get(mem as usize).map(|m| m.buffer.as_slice())
    }

    pub fn read_bytes(&self, mem: u32, addr: u32, len: u32) -> Result<&[u8], HaltError> {
        let start = addr as usize;
        let end = start
            .checked_add(len as usize)
            .ok_or_else(|| trap!("out of bounds memory access"))?;
        self.bytes(mem)
            .ok_or_else(|| anyhow!("no linear memory for index {}", mem))?
            .get(start..end)
            .ok_or_else(|| trap!("out of bounds memory access"))
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