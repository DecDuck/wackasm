use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    module::BlockIndex,
    stack::{BlockAwareStack, MAX_OPERAND_STACK_SIZE, MAX_STACK_POOL, basic::OperandStack},
};

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

    fn recycle_stack(&mut self, stack: OperandStack) {
        if self.stack_pool.len() < MAX_STACK_POOL {
            self.stack_pool.push(stack);
        }
    }
}

impl BlockAwareStack for BlockAwareOperandStack {
    #[inline]
    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        self.get_current_stack().push(value)
    }

    #[inline]
    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), HaltError> {
        self.get_current_stack().push_reversed_all(values)
    }

    #[inline]
    fn pull_reversed_from_stack(&mut self, returns: &[ValType]) -> Result<Vec<Value>, HaltError> {
        self.get_current_stack().pull_reversed_from_stack(returns)
    }

    #[inline]
    fn peek(&mut self) -> Result<&Value, HaltError> {
        self.get_current_stack().peek()
    }

    #[inline]
    fn pop(&mut self) -> Result<Value, HaltError> {
        self.get_current_stack().pop()
    }

    fn push_block(
        &mut self,
        block_idx: BlockIndex,
        pull_from_parenet: Option<&[ValType]>,
    ) -> Result<(), HaltError> {
        let mut stack = self.new_stack();
        if let Some(types) = pull_from_parenet {
            let reversed_parameters = self.get_current_stack().pull_reversed_from_stack(types)?;
            stack.push_reversed_all(reversed_parameters)?;
        }
        self.blocks.push((block_idx, stack));
        Ok(())
    }

    fn peek_block(&self) -> Option<&BlockIndex> {
        self.blocks.last().map(|v| &v.0)
    }

    fn return_block(&mut self, preserve: &[ValType]) -> Result<Option<BlockIndex>, HaltError> {
        let Some((index, mut stack)) = self.blocks.pop() else {
            return Ok(None);
        };

        if preserve.len() > 0 {
            let values = stack.pull_reversed_from_stack(preserve)?;
            self.get_current_stack().push_reversed_all(values)?;
        }

        self.recycle_stack(stack);

        Ok(Some(index))
    }
}
