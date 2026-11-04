use wasmparser::ValType;

use crate::{
    HaltError, Value,
    compiler::DropKeep,
    stack::basic::{LocalOperandStack, OperandStack},
};

mod basic;

pub(crate) type FrameOperandStack = OperandStack;
pub(crate) type StackHandle<'a> = LocalOperandStack<'a>;

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

    /// Pushes all values to the top of the stack
    fn push_all(&mut self, values: &[Value]);
    /// Pulls values from the top of the stack, and returns them.
    fn pull_results(&mut self, types: &[ValType]) -> Result<Vec<Value>, HaltError>;
    /// Equivalent to drop_keep where the drop is enough to move the keep values
    /// down to target
    fn unwind_to(&mut self, target: usize, keep: usize);

    fn reconcile(&mut self, handle: *mut Value);
}

pub(crate) trait RuntimeStack {
    fn into_raw(self) -> *mut Value;
    fn push(&mut self, value: Value);
    fn pop(&mut self) -> Value;

    fn peek(&mut self) -> &Value;
    fn peek_wide(&mut self) -> u128;
    fn peek_mut(&mut self) -> &mut Value;

    /// Copies {keep} number of values to {len - keep - drop} in the stack.
    /// Essentially keeps the top {keep} values while dropping the {drop} values
    /// after that
    fn drop_keep(&mut self, dk: DropKeep);


    #[inline(always)]
    fn pop_i32(&mut self) -> i32 {
        self.pop().0 as i32
    }

    #[inline(always)]
    fn pop_i64(&mut self) -> i64 {
        self.pop().0 as i64
    }

    #[inline(always)]
    fn pop_f32(&mut self) -> f32 {
        self.pop().as_f32()
    }

    #[inline(always)]
    fn pop_f64(&mut self) -> f64 {
        self.pop().as_f64()
    }

    #[inline(always)]
    fn push_v128(&mut self, value: u128) {
        let low = Value(value as u64);
        let high = Value((value >> 64) as u64);
        self.push(low);
        self.push(high);
    }

    #[inline(always)]
    fn pop_v128(&mut self) -> u128 {
        let high = self.pop().0 as u128;
        let low = self.pop().0 as u128;
        (high << 64) | low
    }
}
