use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{BrTable, FunctionBody, MemArg, Operator};

use crate::{HaltError, blocks::BlockManager, pc::ProgramCounter};

mod compile;
mod types;

pub(crate) use types::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WackasmFunctionBody {
    inner: Vec<WackasmOperator>,
}

impl WackasmFunctionBody {
    pub fn empty() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn compile(code: &FunctionBody, blocks: &BlockManager) -> Result<Self, HaltError> {
        let code = code.get_operators_reader()?.into_iter().collect::<Result<Vec<_>, _>>()?;
        compile::compile(code, blocks)
    }

    pub fn get(&self, pc: ProgramCounter) -> &WackasmOperator {
        &self.inner[pc]
    }
}
