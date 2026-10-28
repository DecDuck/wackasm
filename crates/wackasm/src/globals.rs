use std::{collections::HashMap, mem};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{Global as WASMPARSERGlobal, SectionLimited};

use crate::{
    HaltError, InlineWideValue, Value, const_expr::resolve_const_expr, macros::trap,
    module::GlobalIndex,
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Global {
    pub inner: InlineWideValue,
    pub mutable: bool,
    pub shared: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GlobalManager {
    globals: Vec<Global>,
}

impl GlobalManager {
    pub fn new() -> Self {
        Self {
            globals: Vec::new(),
        }
    }

    pub fn process_globals_section(
        &mut self,
        raw_globals: SectionLimited<WASMPARSERGlobal>,
    ) -> Result<(), HaltError> {
        for global in raw_globals {
            let global = global?;
            let init_value = resolve_const_expr(global.init_expr)?;
            let global = Global {
                inner: init_value,
                mutable: global.ty.mutable,
                shared: global.ty.shared,
            };

            self.globals.push(global);
        }

        Ok(())
    }

    pub fn get(&self, global_index: &GlobalIndex) -> Result<&Global, HaltError> {
        self.globals
            .get(global_index.0 as usize)
            .ok_or_else(|| anyhow!("no global for index {:?}", global_index).into())
    }

    pub fn get_wide(&self, global_index: &GlobalIndex) -> Result<u128, HaltError> {
        Ok(self
            .globals
            .get(global_index.0 as usize)
            .ok_or_else(|| trap!("no global for index {:?}", global_index))?
            .inner
            .get_wide()
            .ok_or_else(|| trap!("global not wide"))?)
    }

    pub fn set(&mut self, global_index: &GlobalIndex, new_value: Value) -> Result<(), HaltError> {
        let value = self
            .globals
            .get_mut(global_index.0 as usize)
            .ok_or_else(|| anyhow!("no global for index {:?}", global_index))?;

        value.inner.set(new_value);

        Ok(())
    }

    pub fn set_wide(&mut self, global_index: &GlobalIndex, new_value: u128) -> Result<(), HaltError> {
        let value = self
            .globals
            .get_mut(global_index.0 as usize)
            .ok_or_else(|| anyhow!("no global for index {:?}", global_index))?;

        value.inner.set_wide(new_value);

        Ok(())
    }
}
