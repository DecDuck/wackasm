use serde::{Deserialize, Serialize};
use wasmparser::{
    FuncValidator, FuncValidatorAllocations, FunctionBody, ValType, ValidatorResources,
};

use crate::{HaltError, Value, compiler::compile::Compiler, functions::FunctionManager};

#[path = "filters/block.rs"]
mod block;
#[cfg(test)]
mod comparison_tests;
mod compile;
#[cfg(test)]
mod constant_memory_tests;
mod dispatch;
mod register;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use dispatch::*;
pub(crate) use types::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WackasmFunctionBody {
    pub(crate) inner: Option<Vec<WackasmOperator>>,
    pub(crate) runtime: Vec<RuntimeWackasmOperator>,
    pub(crate) max_height_slots: u32,
    pub(crate) params_slots: usize,
    pub(crate) local_defaults: Box<[Value]>,
}

impl WackasmFunctionBody {
    pub fn empty() -> Self {
        Self {
            runtime: Vec::new(),
            inner: Some(Vec::new()),
            max_height_slots: 0,
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
        compiler.create_locals(params, locals)?;

        // Discover branch targets and optimisation barriers.
        compiler.block_lookahead(code)?;

        // Validate and lower the operand stack to register providers.
        compiler.transform(code, functions)?;

        // Resolve wasm targets to relative instruction offsets.
        compiler.target_rewrite()?;

        // finish up
        compiler.finish()
    }

    pub fn theoretical_max_stack_size(&self) -> u32 {
        self.max_height_slots + self.params_slots as u32 + self.local_defaults.len() as u32
    }
}
