use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value, compiler::DropKeep, macros::trap, module::{BlockIndex, FunctionIndex}, stack::{basic::OperandStack, flat::FlatBlockAwareStack},
};

mod basic;
mod flat;
mod locals;
#[cfg(test)]
mod tests;

const MAX_OPERAND_STACK_SIZE: usize = 200;

pub(crate) use locals::LocalStore;

pub(crate) type FrameOperandStack = FlatBlockAwareStack;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackSection {
    Block(BlockIndex),
    Function(FunctionIndex),
}

impl StackSection {
    pub fn expect_block(&self) -> Result<&BlockIndex, HaltError> {
        match self {
            StackSection::Block(block_index) => Ok(block_index),
            StackSection::Function(_) => Err(trap!("expected block but got function section")),
        }
    }

    pub fn expect_function(&self) -> Result<&FunctionIndex, HaltError> {
        match self {
            StackSection::Block(_) => Err(trap!("expected function but got block section")),
            StackSection::Function(function_index) => Ok(function_index),
        }
    }
}

pub(crate) trait GlobalStack {
    fn clear(&mut self);
    fn push(&mut self, value: Value) -> Result<(), HaltError>;

    fn pop(&mut self) -> Result<Value, HaltError>;

    fn peek(&mut self) -> Result<&Value, HaltError>;
    fn peek_mut(&mut self) -> Result<&mut Value, HaltError>;

    fn drop_keep(&mut self, dk: DropKeep);

    fn push_section(
        &mut self,
        section: StackSection,
        pull_from_parent: Option<&[ValType]>,
    ) -> Result<(), HaltError>;

    fn return_section(&mut self, preserve: &[ValType], pos: Option<usize>) -> Result<Option<StackSection>, HaltError>;
    fn reset_section(&mut self, preserve: &[ValType], pos: usize) -> Result<(), HaltError>;
    fn return_function(&mut self, preserve: &[ValType]) -> Result<Option<StackSection>, HaltError>;

    fn peek_section(&self, pos: Option<usize>) -> Option<&StackSection>;
    fn len_sections(&self) -> usize;

    fn pull_results(&mut self, types: &[ValType]) -> Result<Vec<Value>, HaltError>;

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
