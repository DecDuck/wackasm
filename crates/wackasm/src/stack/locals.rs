use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::{HaltError, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LocalStore {
    inner: Vec<Value>,
    wides: Vec<Option<u64>>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self {
            inner: Vec::new(),
            wides: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn insert(&mut self, index: u32, value: Value) {
        let index = index as usize;
        self.inner.resize_with(index + 1, || Value(u64::MAX));
        self.inner[index] = value;
    }

    pub fn insert_wide(&mut self, index: u32, value: u128) {
        let index = index as usize;
        self.inner.resize_with(index + 1, || Value(u64::MAX));
        self.wides.resize_with(index + 1, || None);
        let high: u64 = (value >> 64) as u64;
        let low: u64 = value as u64;
        self.inner[index] = Value(high);
        self.wides[index] = Some(low);
    }

    pub fn set(&mut self, index: u32, value: Value) -> Result<(), HaltError> {
        let local = self
            .inner
            .get_mut(index as usize)
            .ok_or_else(|| anyhow!("no local for index {}", index))?;

        *local = value;
        Ok(())
    }

    pub fn set_wide(&mut self, index: u32, value: u128) -> Result<(), HaltError> {
        let local = self
            .inner
            .get_mut(index as usize)
            .ok_or_else(|| anyhow!("no local for index {}", index))?;
        let local_wide = self
            .wides
            .get_mut(index as usize)
            .ok_or_else(|| anyhow!("no wide local for index {}", index))?;

        let high: u64 = (value >> 64) as u64;
        let low: u64 = value as u64;

        *local = Value(high);
        *local_wide = Some(low);
        Ok(())
    }

    pub fn get(&self, index: &u32) -> Result<&Value, HaltError> {
        self.inner
            .get(*index as usize)
            .ok_or_else(|| anyhow!("local doesn't exist for index {}", index).into())
    }

    pub fn get_wide(&self, index: &u32) -> Result<u128, HaltError> {
        let high = self
            .inner
            .get(*index as usize)
            .ok_or_else(|| anyhow!("local doesn't exist for index {}", index))?
            .0;
        let low = self.wides.get(*index as usize).unwrap().unwrap();

        let value: u128 = ((high as u128) << 64) | (low as u128);

        Ok(value)
    }
}
