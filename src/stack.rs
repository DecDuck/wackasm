use std::{collections::HashMap, mem::discriminant};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{Value, blocks::Block, module::BlockIndex};

const MAX_OPERAND_STACK_SIZE: usize = 200;
const MAX_STACK_POOL: usize = 10;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
}

impl Clone for OperandStack {
    fn clone(&self) -> Self {
        let mut inner = self.inner.clone();
        let to_reserve = self.inner.capacity().saturating_sub(inner.capacity());
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
    fn pop_i64(&mut self) -> Result<i64, anyhow::Error>;
    fn peek(&mut self) -> Result<&Value, anyhow::Error>;
}

impl OperandStack {
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.inner.clear();
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
                ValType::I64 => Value::I64(self.pop_i64()?),
                ValType::F32 => todo!(),
                ValType::F64 => todo!(),
                ValType::V128 => todo!(),
                ValType::Ref(ref_type) => todo!(),
            });
        }

        Ok(results)
    }

    #[inline]
    fn pop(&mut self) -> Result<Value, anyhow::Error> {
        self.inner.pop().ok_or_else(|| anyhow!("no value in stack"))
    }

    #[inline]
    fn peek(&mut self) -> Result<&Value, anyhow::Error> {
        self.inner
            .last()
            .ok_or_else(|| anyhow!("no value in stack"))
    }

    #[inline]
    fn pop_i32(&mut self) -> Result<i32, anyhow::Error> {
        let value = self.pop()?;
        match value {
            Value::I32(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i32")),
        }
    }

    #[inline]
    fn pop_i64(&mut self) -> Result<i64, anyhow::Error> {
        let value = self.pop()?;
        match value {
            Value::I64(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i64")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BlockAwareOperandStack {
    base: OperandStack,
    blocks: Vec<(BlockIndex, OperandStack)>,
    #[serde(skip)]
    /// Optimisation, keeps some already allocated stacks lying around
    stack_pool: Vec<OperandStack>,
}

impl BlockAwareOperandStack {
    pub fn new(blocks: usize) -> Self {
        Self {
            base: OperandStack::new(),
            blocks: Vec::with_capacity(blocks),
            stack_pool: Vec::with_capacity(MAX_STACK_POOL),
        }
    }

    fn get_current_stack(&mut self) -> &mut OperandStack {
        if let Some(v) = self.blocks.last_mut() {
            &mut v.1
        } else {
            &mut self.base
        }
    }

    fn new_stack(&mut self) -> OperandStack {
        let existing_stack = self.stack_pool.pop();
        if let Some(mut existing_stack) = existing_stack {
            existing_stack.clear();
            return existing_stack;
        }
        OperandStack::new()
    }

    pub fn recycle_stack(&mut self, stack: OperandStack) {
        if self.stack_pool.len() < MAX_STACK_POOL {
            self.stack_pool.push(stack);
        }
    }

    pub fn push_block(
        &mut self,
        block_idx: BlockIndex,
        reversed_parameters: Option<Vec<Value>>,
    ) -> Result<(), anyhow::Error> {
        let mut stack = self.new_stack();
        if let Some(reversed_parameters) = reversed_parameters {
            stack.push_reversed_all(reversed_parameters)?;
        }
        self.blocks.push((block_idx, stack));
        Ok(())
    }

    pub fn pop_block_no_stack(&mut self) -> Option<()> {
        let (_, stack) = self.pop_block()?;
        self.recycle_stack(stack);

        Some(())
    }

    pub fn pop_block(&mut self) -> Option<(BlockIndex, OperandStack)> {
        self.blocks.pop()
    }

    pub fn peek_block(&self) -> Option<&BlockIndex> {
        self.blocks.last().map(|v| &v.0)
    }
}

impl OperandStackFns for BlockAwareOperandStack {
    #[inline]
    fn push(&mut self, value: Value) -> Result<(), anyhow::Error> {
        self.get_current_stack().push(value)
    }

    #[inline]
    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), anyhow::Error> {
        self.get_current_stack().push_reversed_all(values)
    }

    #[inline]
    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, anyhow::Error> {
        self.get_current_stack().pull_reversed_from_stack(returns)
    }

    #[inline]
    fn peek(&mut self) -> Result<&Value, anyhow::Error> {
        self.get_current_stack().peek()
    }

    #[inline]
    fn pop(&mut self) -> Result<Value, anyhow::Error> {
        self.get_current_stack().pop()
    }

    #[inline]
    fn pop_i32(&mut self) -> Result<i32, anyhow::Error> {
        self.get_current_stack().pop_i32()
    }

    #[inline]
    fn pop_i64(&mut self) -> Result<i64, anyhow::Error> {
        self.get_current_stack().pop_i64()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LocalStore {
    inner: Vec<Value>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn insert(&mut self, index: u32, value: Value) {
        let index = index as usize;
        self.inner.resize_with(index + 1, || Value::Unknown);
        self.inner[index] = value;
    }

    #[inline]
    pub fn set_checked(&mut self, index: u32, value: Value) -> Result<(), anyhow::Error> {
        let local = self
            .inner
            .get_mut(index as usize)
            .ok_or_else(|| anyhow!("no local for index {}", index))?;
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

    #[inline]
    pub fn get(&self, index: &u32) -> Result<&Value, anyhow::Error> {
        self.inner
            .get(*index as usize)
            .ok_or_else(|| anyhow!("local doesn't exist for index {}", index))
    }
}
