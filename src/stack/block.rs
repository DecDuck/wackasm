use std::{collections::HashMap, mem::discriminant};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{Value, blocks::Block, module::BlockIndex, stack::{MAX_OPERAND_STACK_SIZE, MAX_STACK_POOL, OperandStackFns, basic::OperandStack}};



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
            base: OperandStack::new(MAX_OPERAND_STACK_SIZE),
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
        OperandStack::new(MAX_OPERAND_STACK_SIZE)
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
