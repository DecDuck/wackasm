use std::{collections::HashMap, mem::discriminant};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{Value, blocks::Block, module::BlockIndex};

const MAX_OPERAND_STACK_SIZE: usize = 200;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
}

impl Clone for OperandStack {
    fn clone(&self) -> Self {
        let mut inner = self.inner.clone();
        let to_reserve = 200usize.saturating_sub(inner.capacity());
        if to_reserve > 0 {
            inner.reserve_exact(to_reserve);
        }
        Self { inner }
    }
}

pub(crate) trait OperandStackFns {
    fn push(&mut self, value: Value) -> Result<(), anyhow::Error>;
    /// Pushes them to the stack in reverse order, so r1 r2 goes onto the stack as [..., r2, r1],
    /// like they were pushed one at a time.
    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), anyhow::Error>;
    /// Pops values from top to bottom, validating them against the result types.
    /// The returned values are therefore in reverse signature order.
    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, anyhow::Error>;
    fn pop(&mut self) -> Result<Value, anyhow::Error>;
    fn pop_i32(&mut self) -> Result<i32, anyhow::Error>;
}

impl OperandStack {
    pub fn new() -> Self {
        Self {
            inner: Vec::with_capacity(MAX_OPERAND_STACK_SIZE),
        }
    }
}

impl OperandStackFns for OperandStack {
    fn push(&mut self, value: Value) -> Result<(), anyhow::Error> {
        if self.inner.len() >= MAX_OPERAND_STACK_SIZE {
            return Err(anyhow!("stack overflow"));
        }
        self.inner.push(value);
        Ok(())
    }

    fn push_reversed_all(&mut self, mut values: Vec<Value>) -> Result<(), anyhow::Error> {
        values.reverse();
        let new_len = self.inner.len().checked_add(values.len());
        if new_len.is_none_or(|len| len > MAX_OPERAND_STACK_SIZE) {
            return Err(anyhow!("stack overflow"));
        }
        self.inner.extend(values);
        Ok(())
    }

    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, anyhow::Error> {
        let mut results = Vec::with_capacity(returns.len());

        for rtn in returns.iter().rev() {
            results.push(match rtn {
                ValType::I32 => Value::I32(self.pop_i32()?),
                ValType::I64 => todo!(),
                ValType::F32 => todo!(),
                ValType::F64 => todo!(),
                ValType::V128 => todo!(),
                ValType::Ref(ref_type) => todo!(),
            });
        }

        Ok(results)
    }

    fn pop(&mut self) -> Result<Value, anyhow::Error> {
        self.inner.pop().ok_or_else(|| anyhow!("no value in stack"))
    }

    fn pop_i32(&mut self) -> Result<i32, anyhow::Error> {
        let value = self.pop()?;
        match value {
            Value::I32(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i32")),
        }
    }
}

struct BlockFrame {
    block_idx: BlockIndex,
    live: OperandStack,
    original: OperandStack,
} 

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BlockAwareOperandStack {
    base: OperandStack,
    blocks: Vec<(BlockIndex, OperandStack)>,
}

impl BlockAwareOperandStack {
    pub fn new(blocks: usize) -> Self {
        Self {
            base: OperandStack::new(),
            blocks: Vec::with_capacity(blocks),
        }
    }

    fn get_current_stack(&mut self) -> &mut OperandStack {
        if let Some(v) = self.blocks.last_mut() {
            &mut v.1
        } else {
            &mut self.base
        }
    }

    pub fn push_block(&mut self, block_idx: BlockIndex, reversed_parameters: Option<Vec<Value>>) -> Result<(), anyhow::Error> {
        let mut stack = OperandStack::new();
        if let Some(reversed_parameters) = reversed_parameters {
            stack.push_reversed_all(reversed_parameters)?;
        }
        self.blocks.push((block_idx, stack));
        Ok(())
    }

    pub fn pop_block(&mut self) -> Option<(BlockIndex, OperandStack)> {
        self.blocks.pop()
    }

    pub fn peek_block(&self) -> Option<&BlockIndex> {
        self.blocks.last().map(|v| &v.0)
    }
}

impl OperandStackFns for BlockAwareOperandStack {
    fn push(&mut self, value: Value) -> Result<(), anyhow::Error> {
        self.get_current_stack().push(value)
    }

    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), anyhow::Error> {
        self.get_current_stack().push_reversed_all(values)
    }

    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, anyhow::Error> {
        self.get_current_stack().pull_reversed_from_stack(returns)
    }

    fn pop(&mut self) -> Result<Value, anyhow::Error> {
        self.get_current_stack().pop()
    }

    fn pop_i32(&mut self) -> Result<i32, anyhow::Error> {
        self.get_current_stack().pop_i32()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LocalStore {
    inner: HashMap<u32, Value>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn insert(&mut self, index: u32, value: Value) {
        self.inner.insert(index, value);
    }

    pub fn set_checked(&mut self, index: u32, value: Value) -> Result<(), anyhow::Error> {
        let local = self
            .inner
            .get_mut(&(index as u32))
            .ok_or(anyhow!("no local for index {}", index))?;
        if discriminant(&value) != discriminant(local) {
            return Err(anyhow!(
                "invalid type for argument {}, expected {:?}",
                index,
                local
            ));
        }
        *local = value;
        Ok(())
    }

    pub fn get(&self, index: &u32) -> Result<&Value, anyhow::Error> {
        self.inner
            .get(index)
            .ok_or_else(|| anyhow!("local doesn't exist for index {}", index))
    }
}
