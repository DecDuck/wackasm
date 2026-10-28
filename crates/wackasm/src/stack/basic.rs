use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value,
    compiler::DropKeep,
    macros::trap,
    stack::{GlobalStack, val_types_to_size},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
    len: usize,
}

impl GlobalStack for OperandStack {
    fn len(&self) -> usize {
        self.len
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push_all(&mut self, values: &[Value]) {
        let end = self.len + values.len();
        self.inner[self.len..end].copy_from_slice(values);
        self.len = end;
    }

    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        self.len += 1;
        self.inner[self.len - 1] = value;
        Ok(())
    }
    fn space(&self) -> usize {
        self.inner.len() - self.len
    }

    fn pop(&mut self) -> Result<Value, HaltError> {
        self.len -= 1;
        Ok(self.inner[self.len]) // - 1 not required to access by index here because we -1 above
    }

    fn peek(&mut self) -> Result<&Value, HaltError> {
        Ok(&self.inner[self.len - 1])
    }
    fn peek_wide(&mut self) -> Result<u128, HaltError> {
        let low = &self.inner[self.len - 2].0;
        let high = &self.inner[self.len - 1].0;
        let mut result = [0u8; 16];
        result[0..8].copy_from_slice(&low.to_le_bytes());
        result[8..].copy_from_slice(&high.to_le_bytes());
        Ok(u128::from_le_bytes(result))
    }

    fn peek_mut(&mut self) -> Result<&mut Value, HaltError> {
        Ok(&mut self.inner[self.len - 1])
    }

    fn drop_keep(&mut self, dk: DropKeep) {
        let (drop, keep) = (dk.drop as usize, dk.keep as usize);
        let len = self.len;
        self.inner.copy_within(len - keep..len, len - keep - drop);
        self.len = len - drop;
    }

    fn pull_results(&mut self, types: &[ValType]) -> Result<Vec<Value>, HaltError> {
        let type_len = val_types_to_size(types);
        let start = self
            .len
            .checked_sub(type_len)
            .ok_or_else(|| trap!("stack underflow"))?;
        let end = self.len;
        self.len = start;
        Ok(self
            .inner
            .get(start..end)
            .ok_or_else(|| trap!("stack under or overflow"))?
            .iter()
            .cloned()
            .collect())
    }

    fn unwind_to(&mut self, target: usize, keep: usize) {
        let len = self.len;
        self.inner.copy_within(len - keep..len, target);
        self.len = target + keep;
    }
    
    fn local_get(&self, base: usize, index: u32) -> Value {
        self.inner[base + index as usize]
    }
    
    fn local_set(&mut self, base: usize, index: u32, value: Value) {
        self.inner[base + index as usize] = value;
    }

    
}

impl OperandStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: vec![Value(u64::MAX); capacity],
            len: 0,
        }
    }
}
