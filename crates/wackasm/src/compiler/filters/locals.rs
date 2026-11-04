use anyhow::anyhow;
use wasmparser::{Operator, ValType, WasmModuleResources as _};

use crate::{
    HaltError,
    compiler::{WackasmOperator, compile::Compiler, filter::CompilerFilter},
    functions::FunctionManager,
    macros::trap,
};

pub struct LocalsPass {}

impl LocalsPass {
    pub fn slot_for_index(&self, compiler: &Compiler, local_index: u32) -> Result<u32, HaltError> {
        Ok(*compiler
            .locals_slot_to_index
            .get(local_index as usize)
            .ok_or_else(|| anyhow!("invalid local index"))? as u32)
    }
}

impl CompilerFilter for LocalsPass {
    fn compile(
        &mut self,
        compiler: &mut Compiler,
        _functions: &FunctionManager,
        _pc: usize,
        operator: Operator,
    ) -> Result<bool, HaltError> {
        let validator = &compiler.func_validator;
        // Gets read the local/global's declared type; everything else consumes the top operand.
        let is_wide = match operator {
            Operator::LocalGet { local_index } => validator.get_local_type(local_index),
            Operator::GlobalGet { global_index } => validator
                .resources()
                .global_at(global_index)
                .map(|global| global.content_type),
            _ => validator.get_operand_type(0).flatten(),
        } == Some(ValType::V128);

        let operator = match operator {
            Operator::LocalGet { local_index } => match is_wide {
                true => WackasmOperator::LocalGetWide {
                    slot: self.slot_for_index(compiler, local_index)?,
                },
                false => WackasmOperator::LocalGet {
                    slot: self.slot_for_index(compiler, local_index)?,
                },
            },
            Operator::LocalSet { local_index } => match is_wide {
                true => WackasmOperator::LocalSetWide {
                    slot: self.slot_for_index(compiler, local_index)?,
                },
                false => WackasmOperator::LocalSet {
                    slot: self.slot_for_index(compiler, local_index)?,
                },
            },
            Operator::Drop => match is_wide {
                true => WackasmOperator::DropWide,
                false => WackasmOperator::Drop,
            },
            Operator::LocalTee { local_index } => match is_wide {
                true => WackasmOperator::LocalTeeWide {
                    slot: self.slot_for_index(compiler, local_index)?,
                },
                false => WackasmOperator::LocalTee {
                    slot: self.slot_for_index(compiler, local_index)?,
                },
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
        compiler.func_body.inner.as_mut().unwrap().push(operator);
        Ok(true)
    }
}

impl LocalsPass {
    pub fn new() -> Self {
        Self {}
    }
}
