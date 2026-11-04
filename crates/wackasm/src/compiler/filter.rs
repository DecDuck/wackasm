use wasmparser::Operator;

use crate::compiler::filters::block::BlockPass;
use crate::compiler::filters::stateless_base::StatelessBasePass;
use crate::functions::FunctionManager;
use crate::{
    HaltError,
    compiler::{WackasmOperator, compile::Compiler},
};

#[static_dispatch::setup]
pub trait CompilerFilter: Send + Sync {
    fn compile(
        &mut self,
        compiler: &mut Compiler,
        functions: &FunctionManager,
        pc: usize,
        operator: Operator,
    ) -> Result<bool, HaltError>;
}

#[static_dispatch::setup]
pub enum CompilerFilterConcrete {
    BlockPass(BlockPass),
    StatelessBase(StatelessBasePass),
}

static_dispatch::implementation!(CompilerFilter for CompilerFilterConcrete);
