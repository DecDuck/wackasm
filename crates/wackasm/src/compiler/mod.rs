use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    BrTable, FuncValidator, FuncValidatorAllocations, FunctionBody, MemArg, Operator,
    ValidatorResources,
};

use crate::{
    HaltError,
    blocks::{BlockManager, lookahead_find_blocks},
    compiler::compile::Compiler,
    functions::FunctionManager,
    pc::ProgramCounter,
};

mod compile;
mod filter;
mod filters;
mod types;

pub(crate) use types::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WackasmFunctionBody {
    pub(crate) inner: Vec<WackasmOperator>,
}

impl WackasmFunctionBody {
    pub fn empty() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn compile(
        code: &FunctionBody,
        func_validator: FuncValidator<ValidatorResources>,
        functions: &FunctionManager,
    ) -> Result<(Self, FuncValidatorAllocations), HaltError> {
        let mut compiler = Compiler::new(func_validator);

        // pass 1: function lookup
        compiler.block_lookahead(code)?;

        // pass 2: transform
        compiler.transform(code, functions)?;

        // pass 3: nop elimination & patch
        compiler.target_rewrite()?;

        // finish up
        compiler.finish()
    }

    pub fn get(&self, pc: ProgramCounter) -> &WackasmOperator {
        &self.inner[pc]
    }
}
