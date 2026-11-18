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
            Operator::MemorySize { mem } => WackasmOperator::MemorySize { mem },
            Operator::MemoryCopy { dst_mem, src_mem } => {
                WackasmOperator::MemoryCopy { dst_mem, src_mem }
            }
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
            Operator::I32Extend8S => WackasmOperator::I32Extend8S,
            Operator::I64ExtendI32U => WackasmOperator::I64ExtendI32U,
            Operator::I32WrapI64 => WackasmOperator::I32WrapI64,
            Operator::I32Extend16S => WackasmOperator::I32Extend16S,
            Operator::I64ExtendI32S => WackasmOperator::I64ExtendI32S,
            Operator::F64ConvertI64S => WackasmOperator::F64ConvertI64S,
            Operator::I64ReinterpretF64 => WackasmOperator::I64ReinterpretF64,
            Operator::F32Add => WackasmOperator::F32Add,
            Operator::F32Sub => WackasmOperator::F32Sub,
            Operator::F32Mul => WackasmOperator::F32Mul,
            Operator::F32Div => WackasmOperator::F32Div,
            Operator::F32Min => WackasmOperator::F32Min,
            Operator::F32Max => WackasmOperator::F32Max,
            Operator::F32Sqrt => WackasmOperator::F32Sqrt,
            Operator::F32Ceil => WackasmOperator::F32Ceil,
            Operator::F32Floor => WackasmOperator::F32Floor,
            Operator::F32Trunc => WackasmOperator::F32Trunc,
            Operator::F32Nearest => WackasmOperator::F32Nearest,
            Operator::F64Add => WackasmOperator::F64Add,
            Operator::F64Sub => WackasmOperator::F64Sub,
            Operator::F64Mul => WackasmOperator::F64Mul,
            Operator::F64Div => WackasmOperator::F64Div,
            Operator::F64Min => WackasmOperator::F64Min,
            Operator::F64Max => WackasmOperator::F64Max,
            Operator::F64Sqrt => WackasmOperator::F64Sqrt,
            Operator::F64Ceil => WackasmOperator::F64Ceil,
            Operator::F64Floor => WackasmOperator::F64Floor,
            Operator::F64Trunc => WackasmOperator::F64Trunc,
            Operator::F64Nearest => WackasmOperator::F64Nearest,
            Operator::F32Eq => WackasmOperator::F32Eq,
            Operator::F32Ne => WackasmOperator::F32Ne,
            Operator::F32Lt => WackasmOperator::F32Lt,
            Operator::F32Gt => WackasmOperator::F32Gt,
            Operator::F32Le => WackasmOperator::F32Le,
            Operator::F32Ge => WackasmOperator::F32Ge,
            Operator::F32Abs => WackasmOperator::F32Abs,
            Operator::F32Neg => WackasmOperator::F32Neg,
            Operator::F32Copysign => WackasmOperator::F32Copysign,
            Operator::F32DemoteF64 => WackasmOperator::F32DemoteF64,
            Operator::F64PromoteF32 => WackasmOperator::F64PromoteF32,
            Operator::I32ReinterpretF32 => WackasmOperator::I32ReinterpretF32,
            Operator::F32ReinterpretI32 => WackasmOperator::F32ReinterpretI32,
            Operator::F32ConvertI32S => WackasmOperator::F32ConvertI32S,
            Operator::F32ConvertI32U => WackasmOperator::F32ConvertI32U,
            Operator::F32ConvertI64S => WackasmOperator::F32ConvertI64S,
            Operator::F32ConvertI64U => WackasmOperator::F32ConvertI64U,
            Operator::F64ConvertI32S => WackasmOperator::F64ConvertI32S,
            Operator::F64ConvertI32U => WackasmOperator::F64ConvertI32U,
            Operator::F64ConvertI64U => WackasmOperator::F64ConvertI64U,
            Operator::I32TruncF32S => WackasmOperator::I32TruncF32S,
            Operator::I32TruncF64S => WackasmOperator::I32TruncF64S,
            Operator::I32TruncSatF32S => WackasmOperator::I32TruncSatF32S,
            Operator::I32TruncSatF64S => WackasmOperator::I32TruncSatF64S,
            Operator::I32TruncF32U => WackasmOperator::I32TruncF32U,
            Operator::I32TruncF64U => WackasmOperator::I32TruncF64U,
            Operator::I32TruncSatF32U => WackasmOperator::I32TruncSatF32U,
            Operator::I64TruncF32S => WackasmOperator::I64TruncF32S,
            Operator::I64TruncF64S => WackasmOperator::I64TruncF64S,
            Operator::I64TruncSatF32S => WackasmOperator::I64TruncSatF32S,
            Operator::I64TruncF32U => WackasmOperator::I64TruncF32U,
            Operator::I64TruncF64U => WackasmOperator::I64TruncF64U,
            Operator::I64TruncSatF32U => WackasmOperator::I64TruncSatF32U,
            Operator::I64TruncSatF64U => WackasmOperator::I64TruncSatF64U,
            Operator::F64Eq => WackasmOperator::F64Eq,
            Operator::F64Ne => WackasmOperator::F64Ne,
            Operator::F64Lt => WackasmOperator::F64Lt,
            Operator::F64Gt => WackasmOperator::F64Gt,
            Operator::F64Le => WackasmOperator::F64Le,
            Operator::F64Ge => WackasmOperator::F64Ge,
            Operator::F64Abs => WackasmOperator::F64Abs,
            Operator::F64Neg => WackasmOperator::F64Neg,
            Operator::F64Copysign => WackasmOperator::F64Copysign,
            Operator::F64ReinterpretI64 => WackasmOperator::F64ReinterpretI64,
            Operator::I32TruncSatF64U => WackasmOperator::I32TruncSatF64U,
            Operator::I64TruncSatF64S => WackasmOperator::I64TruncSatF64S,

            // i32 maths
            Operator::I32Add => WackasmOperator::I32Add,
            Operator::I32Mul => WackasmOperator::I32Mul,
            Operator::I32Ctz => WackasmOperator::I32Ctz,
            Operator::I32DivU => WackasmOperator::I32DivU,
            Operator::I32DivS => WackasmOperator::I32DivS,
            Operator::I32RemS => WackasmOperator::I32RemS,
            Operator::I32RemU => WackasmOperator::I32RemU,
            Operator::I32Clz => WackasmOperator::I32Clz,
            Operator::I32Popcnt => WackasmOperator::I32Popcnt,
            Operator::I32ShrS => WackasmOperator::I32ShrS,
            Operator::I32Sub => WackasmOperator::I32Sub,
            Operator::I32ShrU => WackasmOperator::I32ShrU,
            Operator::I32Shl => WackasmOperator::I32Shl,
            Operator::I32And => WackasmOperator::I32And,
            Operator::I32Xor => WackasmOperator::I32Xor,
            Operator::I32Rotl => WackasmOperator::I32Rotl,
            Operator::I32Rotr => WackasmOperator::I32Rotr,

            // i32 comparators
            Operator::I32GtS => WackasmOperator::I32GtS,
            Operator::I32LtS => WackasmOperator::I32LtS,
            Operator::I32LtU => WackasmOperator::I32LtU,
            Operator::I32GtU => WackasmOperator::I32GtU,
            Operator::I32GeS => WackasmOperator::I32GeS,
            Operator::I32LeS => WackasmOperator::I32LeS,
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
            Operator::I64ShrS => WackasmOperator::I64ShrS,
            Operator::I64DivU => WackasmOperator::I64DivU,
            Operator::I64DivS => WackasmOperator::I64DivS,
            Operator::I64RemS => WackasmOperator::I64RemS,
            Operator::I64RemU => WackasmOperator::I64RemU,
            Operator::I64Clz => WackasmOperator::I64Clz,
            Operator::I64Ctz => WackasmOperator::I64Ctz,
            Operator::I64Popcnt => WackasmOperator::I64Popcnt,
            Operator::I64Rotl => WackasmOperator::I64Rotl,
            Operator::I64Rotr => WackasmOperator::I64Rotr,
            Operator::I64Extend8S => WackasmOperator::I64Extend8S,
            Operator::I64Extend16S => WackasmOperator::I64Extend16S,
            Operator::I64Extend32S => WackasmOperator::I64Extend32S,
            Operator::I64Shl => WackasmOperator::I64Shl,
            Operator::I64And => WackasmOperator::I64And,
            Operator::I64Xor => WackasmOperator::I64Xor,
            Operator::I64Or => WackasmOperator::I64Or,

            // i64 comparators
            Operator::I64GtS => WackasmOperator::I64GtS,
            Operator::I64LtS => WackasmOperator::I64LtS,
            Operator::I64LtU => WackasmOperator::I64LtU,
            Operator::I64GtU => WackasmOperator::I64GtU,
            Operator::I64LeS => WackasmOperator::I64LeS,
            Operator::I64LeU => WackasmOperator::I64LeU,
            Operator::I64GeS => WackasmOperator::I64GeS,
            Operator::I64GeU => WackasmOperator::I64GeU,
            Operator::I64Eq => WackasmOperator::I64Eq,
            Operator::I64Ne => WackasmOperator::I64Ne,
            Operator::I64Eqz => WackasmOperator::I64Eqz,

            _ => WackasmOperator::Unsupported {
                description: Box::new(format!("{:?}", operator)),
            },
        };
        compiler.func_body.inner.as_mut().unwrap().push(compiled_operator);
        return Ok(true);
    }
}

impl StatelessBasePass {
    pub fn new() -> Self {
        Self {}
    }
}
