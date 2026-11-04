use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    BrTable, FuncValidator, FuncValidatorAllocations, FunctionBody, MemArg, Operator, ValType, ValidatorResources,
};

use crate::{
    HaltError, Value, blocks::{BlockManager, lookahead_find_blocks}, compiler::compile::Compiler, functions::FunctionManager, pc::ProgramCounter,
};

mod compile;
mod filter;
mod filters;
mod types;
mod dispatch;

pub(crate) use types::*;
pub(crate) use dispatch::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WackasmFunctionBody {
    pub(crate) inner: Option<Vec<WackasmOperator>>,
    pub(crate) runtime: Vec<RuntimeWackasmOperator>,
    max_stack_operands: u32,
    pub(crate) params_slots: usize,
    pub(crate) local_defaults: Box<[Value]>,

}

impl WackasmFunctionBody {
    pub fn empty() -> Self {
        Self {
            runtime: Vec::new(),
            inner: Some(Vec::new()),
            max_stack_operands: 0,
            params_slots: 0,
            local_defaults: Box::new([]),
        }
    }

    pub fn compile(
        code: &FunctionBody,
        func_validator: FuncValidator<ValidatorResources>,
        functions: &FunctionManager,
        locals: Vec<ValType>,
        params: &[ValType],
    ) -> Result<(Self, FuncValidatorAllocations), HaltError> {
        let mut compiler = Compiler::new(func_validator);

        // pre-pass: create locals + map
        compiler.create_locals(params, locals);

        // pass 1: function lookup
        compiler.block_lookahead(code)?;

        // pass 2: transform
        compiler.transform(code, functions)?;

        // pass 3: nop elimination & patch
        compiler.target_rewrite()?;

        // finish up
        compiler.finish()
    }

    pub fn theoretical_max_stack_size(&self) -> u32 {
        let value = self.max_stack_operands as usize * 2; // v128 takes double size
        let value = value + self.params_slots + self.local_defaults.len();

        value as u32
    }
}
