use wasmparser::Operator;

use crate::{
    HaltError,
    compiler::{WackasmOperator, compile::Compiler, filter::CompilerFilter},
    functions::FunctionManager,
    macros::trap,
};

pub struct LocalsPass {}

impl CompilerFilter for LocalsPass {
    fn compile(
        &mut self,
        compiler: &mut Compiler,
        _functions: &FunctionManager,
        _pc: usize,
        operator: Operator,
    ) -> Result<bool, HaltError> {
        let is_wide = compiler
            .func_validator
            .get_operand_type(0)
            .flatten()
            .map(|val_type| match val_type {
                wasmparser::ValType::V128 => true,
                _ => false,
            })
            .unwrap_or(false);

        let operator = match operator {
            Operator::LocalGet { local_index } => match is_wide {
                true => WackasmOperator::LocalGetWide { local_index },
                false => WackasmOperator::LocalGet { local_index },
            },
            Operator::LocalSet { local_index } => match is_wide {
                true => WackasmOperator::LocalSetWide { local_index },
                false => WackasmOperator::LocalSet { local_index },
            },
            Operator::Drop => match is_wide {
                true => WackasmOperator::DropWide,
                false => WackasmOperator::Drop,
            },
            Operator::LocalTee { local_index } => match is_wide {
                true => WackasmOperator::LocalTeeWide { local_index },
                false => WackasmOperator::LocalTee { local_index },
            },
            Operator::GlobalGet { global_index } => match is_wide {
                true => WackasmOperator::GlobalGetWide { global_index },
                false => WackasmOperator::GlobalGet { global_index },
            },
            Operator::GlobalSet { global_index } => match is_wide {
                true => WackasmOperator::GlobalSetWide { global_index },
                false => WackasmOperator::GlobalSet { global_index },
            },
            _ => return Ok(false),
        };
        compiler.func_body.inner.push(operator);
        Ok(true)
    }
}

impl LocalsPass {
    pub fn new() -> Self {
        Self {}
    }
}
