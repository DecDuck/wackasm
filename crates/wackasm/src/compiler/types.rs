use serde::{Deserialize, Serialize};
use wasmparser::MemArg;

use crate::{HaltError, macros::unsupported};

pub(crate) type Reg = u16;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WackasmMemArg {
    pub(crate) offset: u32,
    pub(crate) memory: u32,
}

impl TryFrom<MemArg> for WackasmMemArg {
    type Error = HaltError;

    fn try_from(value: MemArg) -> Result<Self, Self::Error> {
        Ok(Self {
            offset: u32::try_from(value.offset)
                .map_err(|_| unsupported!("memarg offset {} exceeds 32 bits", value.offset))?,
            memory: value.memory,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum I32Comparison {
    Eq,
    Ne,
    LtS,
    LtU,
    GtS,
    GtU,
    GeS,
    GeU,
    LeU,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum I64Comparison {
    Eq,
    Ne,
    GtS,
    LtU,
    GtU,
    LeU,
    GeU,
}

const _: () = assert!(std::mem::size_of::<WackasmOperator>() <= 16);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WackasmOperator {
    ConsumeFuel,
    Jump {
        offset: i32,
    },
    JumpIfNez {
        cond: Reg,
        offset: i32,
    },
    JumpIfEqz {
        cond: Reg,
        offset: i32,
    },
    JumpIfI32Cmp {
        lhs: Reg,
        rhs: Reg,
        kind: I32Comparison,
        invert: bool,
        offset: i32,
    },
    BrTable {
        index: Reg,
        len: u32,
    },
    Unreachable,
    Nop,
    Return {
        src: Reg,
        len: Reg,
    },
    ReturnIfNez {
        cond: Reg,
        src: Reg,
        len: Reg,
    },
    Select {
        dst: Reg,
        cond: Reg,
        a: Reg,
        b: Reg,
    },
    SelectWide {
        dst: Reg,
        cond: Reg,
        a: Reg,
        b: Reg,
    },
    Call {
        function_index: u32,
        base: Reg,
    },
    CallIndirect {
        index: Reg,
        table_index: u32,
        type_index: u32,
        base: Reg,
    },
    Copy {
        dst: Reg,
        src: Reg,
    },
    CopyWide {
        dst: Reg,
        src: Reg,
    },
    CopySpan {
        dst: Reg,
        src: Reg,
        len: Reg,
    },
    CopyImm32 {
        dst: Reg,
        value: u32,
    },
    CopyImm64 {
        dst: Reg,
        value: [u32; 2],
    },
    GlobalGet {
        dst: Reg,
        global_index: u32,
    },
    GlobalGetWide {
        dst: Reg,
        global_index: u32,
    },
    GlobalSet {
        src: Reg,
        global_index: u32,
    },
    GlobalSetWide {
        src: Reg,
        global_index: u32,
    },
    MemoryGrow {
        dst: Reg,
        amount: Reg,
        mem: u32,
    },
    MemoryCopy {
        dst_mem: u16,
        src_mem: u16,
        dst: Reg,
        src: Reg,
        len: Reg,
    },
    MemoryFill {
        mem: u32,
        addr: Reg,
        value: Reg,
        len: Reg,
    },
    I32Load {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I32Load8U {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I32Load8S {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I32Load16U {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I32Load16S {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load8U {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load8S {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load16U {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load16S {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load32U {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I64Load32S {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    F32Load {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    F64Load {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    V128Load {
        dst: Reg,
        addr: Reg,
        memarg: WackasmMemArg,
    },
    I32Store {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I32Store8 {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I32Store16 {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I64Store {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I64Store8 {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I64Store16 {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I64Store32 {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    F32Store {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    F64Store {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    V128Store {
        addr: Reg,
        value: Reg,
        memarg: WackasmMemArg,
    },
    I32Extend8S {
        dst: Reg,
        src: Reg,
    },
    I64ExtendI32U {
        dst: Reg,
        src: Reg,
    },
    I32WrapI64 {
        dst: Reg,
        src: Reg,
    },
    I32Eqz {
        dst: Reg,
        src: Reg,
    },
    I64Eqz {
        dst: Reg,
        src: Reg,
    },
    I32Add {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Mul {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32DivU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Sub {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32ShrU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Shl {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32And {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Xor {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Rotr {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32GtS {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32LtU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32GtU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32LtS {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32GeS {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32LeU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Ne {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32GeU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Eq {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32Or {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Add {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Mul {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Sub {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64ShrU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Shl {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64And {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Or {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64GtS {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64LtU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64GtU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64LeU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64GeU {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Eq {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I64Ne {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    I32AddImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32SubImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32AndImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32ShlImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32ShrUImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I64AddImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32EqImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32NeImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32LtSImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32LtUImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32GtSImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32GtUImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32GeSImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32GeUImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    I32LeUImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
    },
    Unsupported {
        description: Box<String>,
    },
    JumpIfI32CmpImm {
        lhs: Reg,
        imm: i32,
        kind: I32Comparison,
        invert: bool,
        offset: i32,
    },
    JumpIfI64Cmp {
        lhs: Reg,
        rhs: Reg,
        kind: I64Comparison,
        invert: bool,
        offset: i32,
    },
    JumpIfI64CmpImm {
        lhs: Reg,
        imm: i32,
        kind: I64Comparison,
        invert: bool,
        offset: i32,
    },
    I64CmpImm {
        dst: Reg,
        lhs: Reg,
        imm: i32,
        kind: I64Comparison,
    },
}
