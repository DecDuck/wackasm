use std::{
    hash::{DefaultHasher, Hash as _, Hasher, SipHasher},
    mem::discriminant,
};

use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    macros::trap,
    module::BlockIndex,
    stack::{BlockAwareStack, MAX_OPERAND_STACK_SIZE, basic::OperandStack},
};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FlatBlockAwareStack {
    stack: OperandStack,
    current_floor: usize,
    blocks: Vec<(BlockIndex, usize)>,
}

impl FlatBlockAwareStack {
    pub fn new(blocks: usize) -> Self {
        Self {
            stack: OperandStack::new(MAX_OPERAND_STACK_SIZE * (blocks + 1)),
            current_floor: 0,
            blocks: Vec::with_capacity(blocks),
        }
    }
}

impl BlockAwareStack for FlatBlockAwareStack {
    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        self.stack.push(value)
    }

    fn pop(&mut self) -> Result<Value, HaltError> {
        if self.stack.len() <= self.current_floor {
            return Err(trap!("stack underflow"));
        }

        self.stack.pop()
    }

    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), HaltError> {
        self.stack.push_reversed_all(values)
    }

    fn pull_reversed_from_stack(&mut self, returns: &[ValType]) -> Result<Vec<Value>, HaltError> {
        if self.stack.len().saturating_sub(returns.len()) < self.current_floor {
            return Err(trap!("stack underflow"));
        }

        self.stack.pull_reversed_from_stack(returns)
    }

    fn peek(&mut self) -> Result<&Value, HaltError> {
        if self.stack.len() <= self.current_floor {
            return Err(trap!("stack underflow"));
        }

        self.stack.peek()
    }

    fn push_block(
        &mut self,
        block_idx: BlockIndex,
        pull_from_parent: Option<&[ValType]>,
    ) -> Result<(), crate::HaltError> {
        self.current_floor = self.stack.len() - pull_from_parent.map_or(0, |v| v.len());
        self.blocks.push((block_idx, self.current_floor));

        Ok(())
    }

    fn return_block(&mut self, preserve: &[ValType]) -> Result<Option<BlockIndex>, HaltError> {
        let Some((index, floor)) = self.blocks.pop() else {
            return Ok(None);
        };

        if preserve.len() > 0 {
            if self.stack.len() < floor + preserve.len() {
                return Err(trap!("stack underflow"));
            }

            let source_bottom = self.stack.len() - preserve.len();
            let source_top = self.stack.len();

            self.stack
                .get_mut()
                .copy_within(source_bottom..source_top, floor);
        }

        self.stack.set_len(floor + preserve.len());
        self.current_floor = self.blocks.last().map_or(0, |v| v.1);

        Ok(Some(index))
    }

    fn peek_block(&self) -> Option<&BlockIndex> {
        self.blocks.last().map(|v| &v.0)
    }
}
