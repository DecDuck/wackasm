use std::collections::HashMap;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{Data, DataKind, MemArg, MemoryType, SectionLimited};

use crate::{Value, const_expr::resolve_const_expr, module::MemoryIndex};

pub(crate) trait MemoryNumber: Copy {
    const SIZE: usize;

    fn from_le_slice(bytes: &[u8]) -> Result<Self, anyhow::Error>;
    fn write_le_slice(self, bytes: &mut [u8]);
}

macro_rules! impl_memory_number {
    ($($number:ty),* $(,)?) => {
        $(
            impl MemoryNumber for $number {
                const SIZE: usize = ::core::mem::size_of::<$number>();

                fn from_le_slice(bytes: &[u8]) -> Result<Self, anyhow::Error> {
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

impl_memory_number!(u8, i8, u16, i16, u32, i32, u64, i64, u128, i128, usize, isize, f32, f64);

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
    pub fn new() -> Self {
        Self {
            memories: Vec::new(),
        }
    }

    pub fn process_memory_section(
        &mut self,
        raw_memories: SectionLimited<MemoryType>,
    ) -> Result<(), anyhow::Error> {
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

    pub fn process_data_section(
        &mut self,
        raw_data: SectionLimited<Data>,
    ) -> Result<(), anyhow::Error> {
        for data in raw_data {
            let data = data?;
            match data.kind {
                DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    let offset_value = resolve_const_expr(offset_expr)?;
                    let offset_value = match offset_value {
                        Value::I32(v) => v,
                        _ => return Err(anyhow!("invalid valid {:?} for offset", offset_value)),
                    };

                    let linear_memory = self
                        .memories
                        .get_mut(memory_index as usize)
                        .ok_or_else(|| anyhow!("no linear memory for index {}", memory_index))?;

                    let start = usize::try_from(offset_value as u32)?;
                    let end = start
                        .checked_add(data.data.len())
                        .ok_or_else(|| anyhow!("data segment range overflow"))?;

                    let destination = linear_memory
                        .buffer
                        .get_mut(start..end)
                        .ok_or_else(|| anyhow!("data segment out of bounds"))?;

                    destination.copy_from_slice(data.data);
                }
                DataKind::Passive => todo!(),
            }
        }
        Ok(())
    }

    pub fn grow(&mut self, mem: u32, amount: i32) -> Result<i32, anyhow::Error> {
        let memory = self
            .memories
            .get_mut(mem as usize)
            .ok_or_else(|| anyhow!("memory not found for index {}", mem))?;

        let prev = memory.current;
        let new = prev + amount as u64;
        if let Some(max) = memory.max
            && new > max
        {
            return Ok(-1);
        }

        memory.buffer.resize(new as usize * memory.page_size, 0);
        memory.current = new;

        Ok(prev.try_into()?)
    }

    pub fn fill(
        &mut self,
        mem: u32,
        start: usize,
        length: usize,
        value: u8,
    ) -> Result<(), anyhow::Error> {
        let memory = self
            .memories
            .get_mut(mem as usize)
            .ok_or_else(|| anyhow!("memory not found for index {}", mem))?;

        let end = start
            .checked_add(length)
            .ok_or_else(|| anyhow!("memory.fill out of bounds"))?;

        let destination = memory.buffer.get_mut(start..end).ok_or_else(|| {
            anyhow!(
                "failed to get memory region at {} for {} bytes",
                start,
                length
            )
        })?;
        destination.fill(value);

        Ok(())
    }

    pub(crate) fn load<T: MemoryNumber>(
        &self,
        memarg: &MemArg,
        address: u32,
    ) -> Result<T, anyhow::Error> {
        let memory = self
            .memories
            .get(memarg.memory as usize)
            .ok_or_else(|| anyhow!("no linear memory for index {}", memarg.memory))?;

        let start = usize::try_from(address)?
            .checked_add(usize::try_from(memarg.offset)?)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;
        let end = start
            .checked_add(T::SIZE)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;
        let bytes = memory
            .buffer
            .get(start..end)
            .ok_or_else(|| anyhow!("i32.load out of bounds"))?;

        T::from_le_slice(bytes)
    }

    pub(crate) fn store<T: MemoryNumber>(
        &mut self,
        memarg: &MemArg,
        address: u32,
        value: T,
    ) -> Result<(), anyhow::Error> {
        let memory = self
            .memories
            .get_mut(memarg.memory as usize)
            .ok_or_else(|| anyhow!("no linear memory for index {}", memarg.memory))?;

        let start = usize::try_from(address)?
            .checked_add(usize::try_from(memarg.offset)?)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;
        let end = start
            .checked_add(T::SIZE)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;

        let destination = memory
            .buffer
            .get_mut(start..end)
            .ok_or_else(|| anyhow!("failed to get memory slice"))?;
        value.write_le_slice(destination);

        Ok(())
    }
}
