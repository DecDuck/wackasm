use std::mem::discriminant;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::{HaltError, Value};


#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LocalStore {
    inner: Vec<Value>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn insert(&mut self, index: u32, value: Value) {
        let index = index as usize;
        self.inner.resize_with(index + 1, || Value(u64::MAX));
        self.inner[index] = value;
    }

    #[inline]
    pub fn set_checked(&mut self, index: u32, value: Value) -> Result<(), HaltError> {
        let local = self
            .inner
            .get_mut(index as usize)
            .ok_or_else(|| anyhow!("no local for index {}", index))?;

        *local = value;
        Ok(())
    }

    #[inline]
    pub fn get(&self, index: &u32) -> Result<&Value, HaltError> {
        self.inner
            .get(*index as usize)
            .ok_or_else(|| anyhow!("local doesn't exist for index {}", index).into())
    }
}
