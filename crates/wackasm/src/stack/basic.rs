use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::ValType;

use crate::{
    HaltError, Value, macros::trap, module::BlockIndex, stack::BlockAwareStack,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct OperandStack {
    inner: Vec<Value>,
    len: usize,
}

impl BlockAwareStack for OperandStack {
    fn push(&mut self, value: Value) -> Result<(), HaltError> {
        if self.len == self.inner.len() {
            return Err(trap!("stack overflow"));
        }

        self.len += 1;
        self.inner[self.len - 1] = value;
        Ok(())
    }

    fn push_reversed_all(&mut self, mut values: Vec<Value>) -> Result<(), HaltError> {
        if values.is_empty() {
            return Ok(());
        }

        values.reverse();
        let start = self.len;
        let end = start
            .checked_add(values.len())
            .ok_or_else(|| trap!("stack overflow"))?;

        if end > self.inner.len() {
            return Err(trap!("stack overflow"));
        }

        let destination = self
            .inner
            .get_mut(start..end)
            .ok_or_else(|| trap!("stack overflow"))?;
        destination.copy_from_slice(&values);

        self.len = end;

        Ok(())
    }

    fn pull_reversed_from_stack(
        &mut self,
        returns: &[ValType],
    ) -> Result<Vec<Value>, HaltError> {
        let mut results = Vec::with_capacity(returns.len());

        for rtn in returns.iter().rev() {
            results.push(self.pop()?);
        }

        Ok(results)
    }

    #[inline]
    fn pop(&mut self) -> Result<Value, HaltError> {
        if self.len == 0 {
            return Err(anyhow!("stack underflow").into());
        }

        self.len -= 1;
        Ok(self.inner[self.len]) // - 1 not required to access by index here because we -1 above
    }

    #[inline]
    fn peek(&mut self) -> Result<&Value, HaltError> {
        if self.len == 0 {
            return Err(anyhow!("stack underflow").into());
        }
        Ok(&self.inner[self.len - 1])
    }
    
    // We don't implement block functions
    fn push_block(
        &mut self,
        block_idx: crate::module::BlockIndex,
        reversed_parameters: Option<Vec<Value>>,
    ) -> Result<(), HaltError> {
        todo!()
    }
    fn return_block(
        &mut self,
        preserve: &[ValType],
    ) -> Result<Option<BlockIndex>, HaltError> {
        todo!()
    }
    fn peek_block(&self) -> Option<&crate::module::BlockIndex> {
        todo!()
    }


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