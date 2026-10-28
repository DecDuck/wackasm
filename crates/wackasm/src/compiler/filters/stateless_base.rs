use anyhow::anyhow;
use wasmparser::Operator;

use crate::{
    HaltError,
    compiler::{WackasmOperator, compile::Compiler, filter::CompilerFilter},
    functions::FunctionManager,
    stack::val_type_to_size,
};

pub struct StatelessBasePass {}

impl CompilerFilter for StatelessBasePass {
    fn compile(
        &mut self,
        compiler: &mut Compiler,
        _functions: &FunctionManager,
        _pc: usize,
        operator: wasmparser::Operator,
    ) -> Result<bool, HaltError> {
        let compiled_operator = match operator {
            Operator::Nop => return Ok(true),

            // Control
            Operator::Unreachable => WackasmOperator::Unreachable,
            Operator::Return => WackasmOperator::Return,
            Operator::Select => {
                let wide = val_type_to_size(
                    &compiler
                        .func_validator
                        .get_operand_type(1)
                        .flatten()
                        .ok_or_else(|| anyhow!("failed to inspect select top value"))?,
                ) > 1;
                WackasmOperator::Select { wide }
            }
            Operator::Call { function_index } => WackasmOperator::Call { function_index },
            Operator::CallIndirect {
                table_index,
                type_index,
                ..
            } => WackasmOperator::CallIndirect {
                table_index,
                type_index,
            },

            // Memory
            Operator::MemoryGrow { mem } => WackasmOperator::MemoryGrow { mem },
            Operator::MemoryFill { mem } => WackasmOperator::MemoryFill { mem },

            // Loads
            Operator::I32Load { memarg } => WackasmOperator::I32Load {
                memarg: memarg.try_into()?,
            },
            Operator::I32Load8U { memarg } => WackasmOperator::I32Load8U {
                memarg: memarg.try_into()?,
            },
            Operator::I32Load8S { memarg } => WackasmOperator::I32Load8S {
                memarg: memarg.try_into()?,
            },
            Operator::I32Load16U { memarg } => WackasmOperator::I32Load16U {
                memarg: memarg.try_into()?,
            },
            Operator::I32Load16S { memarg } => WackasmOperator::I32Load16S {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load { memarg } => WackasmOperator::I64Load {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load8U { memarg } => WackasmOperator::I64Load8U {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load8S { memarg } => WackasmOperator::I64Load8S {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load16U { memarg } => WackasmOperator::I64Load16U {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load16S { memarg } => WackasmOperator::I64Load16S {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load32U { memarg } => WackasmOperator::I64Load32U {
                memarg: memarg.try_into()?,
            },
            Operator::I64Load32S { memarg } => WackasmOperator::I64Load32S {
                memarg: memarg.try_into()?,
            },
            Operator::F32Load { memarg } => WackasmOperator::F32Load {
                memarg: memarg.try_into()?,
            },
            Operator::F64Load { memarg } => WackasmOperator::F64Load {
                memarg: memarg.try_into()?,
            },
            Operator::V128Load { memarg } => WackasmOperator::V128Load {
                memarg: memarg.try_into()?,
            },

            // Stores
            Operator::I32Store { memarg } => WackasmOperator::I32Store {
                memarg: memarg.try_into()?,
            },
            Operator::I32Store8 { memarg } => WackasmOperator::I32Store8 {
                memarg: memarg.try_into()?,
            },
            Operator::I32Store16 { memarg } => WackasmOperator::I32Store16 {
                memarg: memarg.try_into()?,
            },
            Operator::I64Store { memarg } => WackasmOperator::I64Store {
                memarg: memarg.try_into()?,
            },
            Operator::I64Store8 { memarg } => WackasmOperator::I64Store8 {
                memarg: memarg.try_into()?,
            },
            Operator::I64Store16 { memarg } => WackasmOperator::I64Store16 {
                memarg: memarg.try_into()?,
            },
            Operator::I64Store32 { memarg } => WackasmOperator::I64Store32 {
                memarg: memarg.try_into()?,
            },
            Operator::F32Store { memarg } => WackasmOperator::F32Store {
                memarg: memarg.try_into()?,
            },
            Operator::F64Store { memarg } => WackasmOperator::F64Store {
                memarg: memarg.try_into()?,
            },
            Operator::V128Store { memarg } => WackasmOperator::V128Store {
                memarg: memarg.try_into()?,
            },

            // Maths
            Operator::I32Const { value } => WackasmOperator::I32Const { value },
            Operator::I64Const { value } => WackasmOperator::I64Const { value },
            Operator::F32Const { value } => WackasmOperator::F32Const {
                value: f32::from_bits(value.bits()),
            },
            Operator::F64Const { value } => WackasmOperator::F64Const {
                value: f64::from_bits(value.bits()),
            },
            Operator::I64ExtendI32U => WackasmOperator::I64ExtendI32U,
            Operator::I32WrapI64 => WackasmOperator::I32WrapI64,

            // i32 maths
            Operator::I32Add => WackasmOperator::I32Add,
            Operator::I32Mul => WackasmOperator::I32Mul,
            Operator::I32DivU => WackasmOperator::I32DivU,
            Operator::I32Sub => WackasmOperator::I32Sub,
            Operator::I32ShrU => WackasmOperator::I32ShrU,
            Operator::I32Shl => WackasmOperator::I32Shl,
            Operator::I32And => WackasmOperator::I32And,
            Operator::I32Xor => WackasmOperator::I32Xor,

            // i32 comparators
            Operator::I32GtS => WackasmOperator::I32GtS,
            Operator::I32LtS => WackasmOperator::I32LtS,
            Operator::I32LtU => WackasmOperator::I32LtU,
            Operator::I32GtU => WackasmOperator::I32GtU,
            Operator::I32LeU => WackasmOperator::I32LeU,
            Operator::I32Ne => WackasmOperator::I32Ne,
            Operator::I32GeU => WackasmOperator::I32GeU,
            Operator::I32Eqz => WackasmOperator::I32Eqz,
            Operator::I32Eq => WackasmOperator::I32Eq,
            Operator::I32Or => WackasmOperator::I32Or,

            // i64 maths
            Operator::I64Add => WackasmOperator::I64Add,
            Operator::I64Mul => WackasmOperator::I64Mul,
            Operator::I64Sub => WackasmOperator::I64Sub,
            Operator::I64ShrU => WackasmOperator::I64ShrU,
            Operator::I64Shl => WackasmOperator::I64Shl,
            Operator::I64And => WackasmOperator::I64And,
            Operator::I64Or => WackasmOperator::I64Or,

            // i64 comparators
            Operator::I64GtS => WackasmOperator::I64GtS,
            Operator::I64LtU => WackasmOperator::I64LtU,
            Operator::I64GtU => WackasmOperator::I64GtU,
            Operator::I64LeU => WackasmOperator::I64LeU,
            Operator::I64GeU => WackasmOperator::I64GeU,
            Operator::I64Eq => WackasmOperator::I64Eq,
            Operator::I64Ne => WackasmOperator::I64Ne,
            Operator::I64Eqz => WackasmOperator::I64Eqz,

            _ => WackasmOperator::Unsupported {
                description: Box::new(format!("{:?}", operator)),
            },
        };
        compiler.func_body.inner.push(compiled_operator);
        return Ok(true);
    }
}

impl StatelessBasePass {
    pub fn new() -> Self {
        Self {}
    }
}
