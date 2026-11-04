use anyhow::anyhow;
use wasmparser::ValType;

use crate::{HaltError, Value, module::BlockIndex, stack::{basic::OperandStack, flat::FlatBlockAwareStack}};

mod basic;
mod block;
mod locals;
mod flat;
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

    fn return_block(
        &mut self,
        preserve: &[ValType],
    ) -> Result<Option<BlockIndex>, HaltError>;
    fn peek_block(&self) -> Option<&BlockIndex>;

    fn pop_i32(&mut self) -> Result<i32, HaltError> {
        let value = self.pop()?;
        match value {
            Value::I32(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i32").into()),
        }
    }

    fn pop_i64(&mut self) -> Result<i64, HaltError> {
        let value = self.pop()?;
        match value {
            Value::I64(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected i64").into()),
        }
    }

    fn pop_f32(&mut self) -> Result<f32, HaltError> {
        let value = self.pop()?;
        match value {
            Value::F32(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected f32").into()),
        }
    }

    fn pop_f64(&mut self) -> Result<f64, HaltError> {
        let value = self.pop()?;
        match value {
            Value::F64(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected f64").into()),
        }
    }

    fn pop_v128(&mut self) -> Result<u128, HaltError> {
        let value = self.pop()?;
        match value {
            Value::V128(v) => Ok(v),
            _ => Err(anyhow!("invalid type, expected v128").into()),
        }
    }
}
