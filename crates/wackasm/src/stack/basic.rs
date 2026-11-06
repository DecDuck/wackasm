use serde::{Deserialize, Serialize};

use crate::{HaltError, Value, macros::trap};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
}

impl OperandStack {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: vec![Value(0); capacity],
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut Value {
        self.inner.as_mut_ptr()
    }

    pub(crate) fn read(&self, base: usize, len: usize) -> Result<&[Value], HaltError> {
        let end = base
            .checked_add(len)
            .ok_or_else(|| trap!("stack overflow"))?;
        self.inner
            .get(base..end)
            .ok_or_else(|| trap!("stack under or overflow"))
    }

    pub(crate) fn write(&mut self, base: usize, values: &[Value]) -> Result<(), HaltError> {
        let end = base
            .checked_add(values.len())
            .ok_or_else(|| trap!("stack overflow"))?;
        self.inner
            .get_mut(base..end)
            .ok_or_else(|| trap!("stack under or overflow"))?
            .copy_from_slice(values);
        Ok(())
    }
}
