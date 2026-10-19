use std::collections::HashMap;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{Data, DataKind, MemArg, MemoryType, SectionLimited};

use crate::{Value, const_expr::resolve_const_expr, module::MemoryIndex};

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
                        .ok_or(anyhow!("no linear memory for index {}", memory_index))?;

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

        let destination = memory.buffer.get_mut(start..end).ok_or(anyhow!(
            "failed to get memory region at {} for {} bytes",
            start,
            length
        ))?;
        destination.fill(value);

        Ok(())
    }

    pub fn load_i32<const SIZE: usize>(
        &self,
        memarg: &MemArg,
        address: u32,
    ) -> Result<i32, anyhow::Error> {
        let memory = self
            .memories
            .get(memarg.memory as usize)
            .ok_or_else(|| anyhow!("no linear memory for index {}", memarg.memory))?;

        let start = usize::try_from(address)?
            .checked_add(usize::try_from(memarg.offset)?)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;
        let end = start
            .checked_add(SIZE)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;
        let bytes = memory
            .buffer
            .get(start..end)
            .ok_or_else(|| anyhow!("i32.load out of bounds"))?;

        let mut fixed_size = [0u8; 4];
        fixed_size[..bytes.len()].copy_from_slice(bytes);
        let value = i32::from_le_bytes(fixed_size);

        Ok(value)
    }

    pub fn store_i32<const SIZE: usize>(
        &mut self,
        memarg: &MemArg,
        address: u32,
        value: i32,
    ) -> Result<(), anyhow::Error> {
        let memory = self
            .memories
            .get_mut(memarg.memory as usize)
            .ok_or_else(|| anyhow!("no linear memory for index {}", memarg.memory))?;

        let start = usize::try_from(address)?
            .checked_add(usize::try_from(memarg.offset)?)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;
        let end = start
            .checked_add(SIZE)
            .ok_or_else(|| anyhow!("i32.load address overflow"))?;

        let encoded = value.to_le_bytes();

        let destination = memory
            .buffer
            .get_mut(start..end)
            .ok_or_else(|| anyhow!("failed to get memory slice"))?;
        destination.copy_from_slice(&encoded[0..SIZE]);

        Ok(())
    }
}
