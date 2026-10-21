use anyhow::anyhow;
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    module::BlockIndex,
    stack::{basic::OperandStack, flat::FlatBlockAwareStack},
};

mod basic;
mod block;
mod flat;
mod locals;
#[cfg(test)]
mod tests;

const MAX_OPERAND_STACK_SIZE: usize = 200;
const MAX_STACK_POOL: usize = 10;

use block::BlockAwareOperandStack;
pub(crate) use locals::LocalStore;

pub(crate) type FrameOperandStack = FlatBlockAwareStack;

pub(crate) trait BlockAwareStack {
    fn push(&mut self, value: Value) -> Result<(), HaltError>;
    /// Pushes them to the stack in reverse order, so r1 r2 goes onto the stack as [..., r2, r1],
    /// like they were pushed one at a time.
    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), HaltError>;
    /// Pops values from top to bottom, validating them against the result types.
    /// The returned values are therefore in reverse signature order.
    fn pull_reversed_from_stack(&mut self, returns: &[ValType]) -> Result<Vec<Value>, HaltError>;
    fn pop(&mut self) -> Result<Value, HaltError>;

    fn peek(&mut self) -> Result<&Value, HaltError>;

    fn push_block(
        &mut self,
        block_idx: BlockIndex,
        reversed_parameters: Option<Vec<Value>>,
    ) -> Result<(), HaltError>;

    fn return_block(&mut self, preserve: &[ValType]) -> Result<Option<BlockIndex>, HaltError>;
    fn peek_block(&self) -> Option<&BlockIndex>;

    fn pop_i32(&mut self) -> Result<i32, HaltError> {
        Ok(self.pop()?.0 as i32)
    }

    fn pop_i64(&mut self) -> Result<i64, HaltError> {
        Ok(self.pop()?.0 as i64)
    }

    fn pop_f32(&mut self) -> Result<f32, HaltError> {
        Ok(self.pop()?.as_f32())
    }

    fn pop_f64(&mut self) -> Result<f64, HaltError> {
        Ok(self.pop()?.as_f64())
    }

    fn push_v128(&mut self, value: u128) -> Result<(), HaltError> {
        let bytes = value.to_le_bytes();
        let low = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let high = u64::from_le_bytes(bytes[8..].try_into().unwrap());

        self.push(Value(high))?;
        self.push(Value(low))?;

        Ok(())
    }

    fn pop_v128(&mut self) -> Result<u128, HaltError> {
        let low = self.pop()?.0;
        let high = self.pop()?.0;
        let mut result = [0u8; 16];
        result[0..8].copy_from_slice(&low.to_le_bytes());
        result[8..].copy_from_slice(&high.to_le_bytes());
        Ok(u128::from_le_bytes(result))
    }
}
