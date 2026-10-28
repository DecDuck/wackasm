use wasmparser::ValType;

use crate::{HaltError, Value, compiler::DropKeep, stack::basic::OperandStack};

mod basic;

pub(crate) type FrameOperandStack = OperandStack;

pub fn val_type_to_size(val_type: &ValType) -> usize {
    if matches!(val_type, &ValType::V128) {
        2
    } else {
        1
    }
}
pub fn val_types_to_size(val_types: &[ValType]) -> usize {
    val_types
        .iter()
        .map(|t| if matches!(t, ValType::V128) { 2 } else { 1 })
        .sum()
}

pub(crate) trait GlobalStack {
    fn len(&self) -> usize;
    fn space(&self) -> usize;

    fn clear(&mut self);
    fn push(&mut self, value: Value) -> Result<(), HaltError>;
    fn pop(&mut self) -> Result<Value, HaltError>;

    fn peek(&mut self) -> Result<&Value, HaltError>;
    fn peek_wide(&mut self) -> Result<u128, HaltError>;
    fn peek_mut(&mut self) -> Result<&mut Value, HaltError>;

    fn push_all(&mut self, values: &[Value]);

    fn local_get(&self, base: usize, index: u32) -> Value;
    fn local_set(&mut self, base: usize, index: u32, value: Value);
    fn local_wide_get(&self, base: usize, index: u32) -> u128 {
        let low = self.local_get(base, index).0 as u128;
        let high = self.local_get(base, index + 1).0 as u128;
        (high << 64) | low
    }
    fn local_wide_set(&mut self, base: usize, index: u32, value: u128) {
        let low = Value(value as u64);
        let high = Value((value >> 64) as u64);
        self.local_set(base, index, low);
        self.local_set(base, index + 1, high);
    }

    /// Copies {keep} number of values to {len - keep - drop} in the stack.
    /// Essentially keeps the top {keep} values while dropping the {drop} values
    /// after that
    fn drop_keep(&mut self, dk: DropKeep);
    /// Equivalent to drop_keep where the drop is enough to move the keep values
    /// down to target
    fn unwind_to(&mut self, target: usize, keep: usize);

    /// Pulls values from the top of the stack, and returns them.
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
        let low = Value(value as u64);
        let high = Value((value >> 64) as u64);
        self.push(low)?;
        self.push(high)?;
        Ok(())
    }

    fn pop_v128(&mut self) -> Result<u128, HaltError> {
        let high = self.pop()?.0 as u128;
        let low = self.pop()?.0 as u128;
        Ok((high << 64) | low)
    }
}
