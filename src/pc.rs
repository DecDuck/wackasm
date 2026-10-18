use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[repr(transparent)]
#[serde(transparent)]
pub(crate) struct ProgramCounter {
    inner: usize,
}

impl ProgramCounter {
    pub fn new() -> Self {
        Self { inner: 0 }
    }

    pub fn handle<'a>(&'a mut self) -> ProgramCounterHandle<'a> {
        ProgramCounterHandle { target: &mut self.inner, overrride: None }
    }

    #[inline]
    pub fn inner(&self) -> usize {
        self.inner
    }
}

pub(crate) struct ProgramCounterHandle<'a> {
    target: &'a mut usize,
    overrride: Option<usize>,
}

impl<'a> ProgramCounterHandle<'a> {
    #[inline]
    pub fn inner(&self) -> usize {
        *self.target
    }

    pub fn set_pc(&mut self, target: usize) {
        self.overrride = Some(target);
    }
}

impl<'a> Drop for ProgramCounterHandle<'a> {
    fn drop(&mut self) {
        if let Some(overrride) = self.overrride {
            *self.target = overrride;
        } else {
            *self.target += 1;
        }
    }
}