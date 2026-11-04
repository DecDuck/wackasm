use wasmparser::ValType;

use crate::{HaltError, Value};

mod basic;
mod block;
mod locals;

const MAX_OPERAND_STACK_SIZE: usize = 200;
const MAX_STACK_POOL: usize = 10;

pub(crate) use basic::OperandStack;
pub(crate) use block::BlockAwareOperandStack;
pub(crate) use locals::LocalStore;

pub(crate) trait OperandStackFns {
    fn push(&mut self, value: Value) -> Result<(), HaltError>;
    /// Pushes them to the stack in reverse order, so r1 r2 goes onto the stack as [..., r2, r1],
    /// like they were pushed one at a time.
    fn push_reversed_all(&mut self, values: Vec<Value>) -> Result<(), HaltError>;
    /// Pops values from top to bottom, validating them against the result types.
    /// The returned values are therefore in reverse signature order.
    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, HaltError>;
    fn pop(&mut self) -> Result<Value, HaltError>;
    fn pop_i32(&mut self) -> Result<i32, HaltError>;
    fn pop_i64(&mut self) -> Result<i64, HaltError>;
    fn pop_f32(&mut self) -> Result<f32, HaltError>;
    fn pop_f64(&mut self) -> Result<f64, HaltError>;
    fn pop_v128(&mut self) -> Result<u128, HaltError>;
    fn peek(&mut self) -> Result<&Value, HaltError>;
}
