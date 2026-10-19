use std::{collections::HashMap, mem};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{Global as WASMPARSERGlobal, SectionLimited};

use crate::{Value, const_expr::resolve_const_expr, module::GlobalIndex};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Global {
    pub inner: Value,
    pub mutable: bool,
    pub shared: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GlobalManager {
    pub(crate) globals: HashMap<GlobalIndex, Global>,
}

impl GlobalManager {
    pub fn new() -> Self {
        Self {
            globals: HashMap::new(),
        }
    }

    pub fn process_globals_section(
        &mut self,
        raw_globals: SectionLimited<WASMPARSERGlobal>,
    ) -> Result<(), anyhow::Error> {
        for (global_idx, global) in raw_globals.into_iter().enumerate() {
            let global = global?;
            let init_value = resolve_const_expr(global.init_expr)?;
            let global = Global {
                inner: init_value,
                mutable: global.ty.mutable,
                shared: global.ty.shared,
            };

            self.globals.insert(GlobalIndex(global_idx as u32), global);
        }

        Ok(())
    }

    pub fn get(&self, global_index: &GlobalIndex) -> Result<&Global, anyhow::Error> {
        self.globals
            .get(global_index)
            .ok_or_else(|| anyhow!("no global for index {:?}", global_index))
    }

    pub fn set(
        &mut self,
        global_index: &GlobalIndex,
        new_value: Value,
    ) -> Result<(), anyhow::Error> {
        let value = self
            .globals
            .get_mut(global_index)
            .ok_or_else(|| anyhow!("no global for index {:?}", global_index))?;

        // Check discriminate
        if mem::discriminant(&value.inner) != mem::discriminant(&new_value) {
            return Err(anyhow!("global value types mismatched"));
        }

        value.inner = new_value;

        Ok(())
    }
}
