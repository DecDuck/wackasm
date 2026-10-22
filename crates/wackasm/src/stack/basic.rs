use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{HaltError, Value, macros::trap, module::BlockIndex, stack::GlobalStack};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
    len: usize,
}

impl GlobalStack for OperandStack {
    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        if self.len == self.inner.len() {
            return Err(trap!("stack overflow"));
        }

        self.len += 1;
        self.inner[self.len - 1] = value;
        Ok(())
    }

    #[inline]
    fn pop(&mut self) -> Result<Value, HaltError> {
        self.len -= 1;
        Ok(self.inner[self.len]) // - 1 not required to access by index here because we -1 above
    }

    #[inline]
    fn peek(&mut self) -> Result<&Value, HaltError> {
        Ok(&self.inner[self.len - 1])
    }

    fn pull_results(&mut self, types: &[ValType]) -> Result<Vec<Value>, HaltError> {
        let start = self
            .len()
            .checked_sub(types.len())
            .ok_or_else(|| trap!("stack underflow"))?;
        let end = self.len();
        self.len = start;
        Ok(self
            .inner
            .get(start..end)
            .ok_or_else(|| trap!("stack under or overflow"))?
            .iter()
            .cloned()
            .collect())
    }

    fn push_section(
        &mut self,
        section: super::StackSection,
        pull_from_parent: Option<&[ValType]>,
    ) -> Result<(), HaltError> {
        todo!()
    }

    fn return_section(
        &mut self,
        preserve: &[ValType],
        position: Option<usize>,
    ) -> Result<Option<super::StackSection>, HaltError> {
        todo!()
    }

    fn reset_section(&mut self, preserve: &[ValType], pos: usize) -> Result<(), HaltError> {
        todo!()
    }

    fn peek_section(&self, position: Option<usize>) -> Option<&super::StackSection> {
        todo!()
    }

    fn return_function(
        &mut self,
        preserve: &[ValType],
    ) -> Result<Option<super::StackSection>, HaltError> {
        todo!()
    }

    fn len_sections(&self) -> usize {
        todo!()
    }

    // We don't implement block functions
}

impl OperandStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: vec![Value(u64::MAX); capacity],
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get_mut(&mut self) -> &mut Vec<Value> {
        &mut self.inner
    }
}
