use serde::{Deserialize, Serialize};
use wasmparser::{BlockType, BrTable, MemArg};

use crate::{
    HaltError,
    macros::unsupported,
    module::{BlockIndex, FunctionTypeIndex},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WackasmMemArg {
    pub(crate) offset: u32,
    pub(crate) memory: u32,
}

impl TryFrom<MemArg> for WackasmMemArg {
    type Error = HaltError;

    fn try_from(value: MemArg) -> Result<Self, Self::Error> {
        Ok(WackasmMemArg {
            offset: u32::try_from(value.offset)
                .map_err(|_| unsupported!("memarg offset {} exceeds 32 bits", value.offset))?,
            memory: value.memory,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WackasmTableTarget {
    Return,
    Break { target: u32, dk: DropKeep },
}

pub type WackasmTableTargets = Vec<WackasmTableTarget>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BlockParams(u32);

impl BlockParams {
    pub const NONE: Self = Self(u32::MAX);

    pub fn from_block_type(ty: &BlockType) -> Self {
        match ty {
            BlockType::FuncType(i) => Self(*i),
            _ => Self::NONE,
        }
    }

    pub fn get(self) -> Option<FunctionTypeIndex> {
        (self.0 != u32::MAX).then_some(FunctionTypeIndex(self.0))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DropKeep {
    pub drop: u16,
    pub keep: u16,
}

const _: () = assert!(std::mem::size_of::<WackasmOperator>() <= 16);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WackasmOperator {
    ConsumeFuel,

    // Compiled block handling
    Jump {
        target: u32,
    },
    JumpIf {
        target: u32,
    },
    JumpIfEqz {
        target: u32,
    },

    BrIf {
        target: u32,
        drop_keep: DropKeep,
    },
    Br {
        target: u32,
        drop_keep: DropKeep,
    },
    BrTable {
        targets: Box<WackasmTableTargets>,
    },

    // Control
    Unreachable,
    Nop,
    Return,
    ReturnIf,
    Select { wide: bool },
    Call {
        function_index: u32,
    },
    CallIndirect {
        table_index: u32,
        type_index: u32,
    },

    // Stack
    LocalGet {
        slot: u32,
    },
    LocalGetWide {
        slot: u32,
    },
    LocalSet {
        slot: u32,
    },
    LocalSetWide {
        slot: u32,
    },
    Drop,
    DropWide,
    LocalTee {
        slot: u32,
    },
    LocalTeeWide {
        slot: u32,
    },
    GlobalGet {
        global_index: u32,
    },
    GlobalGetWide {
        global_index: u32,
    },
    GlobalSet {
        global_index: u32,
    },
    GlobalSetWide {
        global_index: u32,
    },

    // Memory
    MemoryGrow {
        mem: u32,
    },
    MemoryFill {
        mem: u32,
    },

    // Loads
    I32Load {
        memarg: WackasmMemArg,
    },
    I32Load8U {
        memarg: WackasmMemArg,
    },
    I32Load8S {
        memarg: WackasmMemArg,
    },
    I32Load16U {
        memarg: WackasmMemArg,
    },
    I32Load16S {
        memarg: WackasmMemArg,
    },

    I64Load {
        memarg: WackasmMemArg,
    },
    I64Load8U {
        memarg: WackasmMemArg,
    },
    I64Load8S {
        memarg: WackasmMemArg,
    },
    I64Load16U {
        memarg: WackasmMemArg,
    },
    I64Load16S {
        memarg: WackasmMemArg,
    },
    I64Load32U {
        memarg: WackasmMemArg,
    },
    I64Load32S {
        memarg: WackasmMemArg,
    },

    F32Load {
        memarg: WackasmMemArg,
    },
    F64Load {
        memarg: WackasmMemArg,
    },
    V128Load {
        memarg: WackasmMemArg,
    },

    // Stores
    I32Store {
        memarg: WackasmMemArg,
    },
    I32Store8 {
        memarg: WackasmMemArg,
    },
    I32Store16 {
        memarg: WackasmMemArg,
    },

    I64Store {
        memarg: WackasmMemArg,
    },
    I64Store8 {
        memarg: WackasmMemArg,
    },
    I64Store16 {
        memarg: WackasmMemArg,
    },
    I64Store32 {
        memarg: WackasmMemArg,
    },

    F32Store {
        memarg: WackasmMemArg,
    },
    F64Store {
        memarg: WackasmMemArg,
    },
    V128Store {
        memarg: WackasmMemArg,
    },

    // Maths
    I32Const {
        value: i32,
    },
    I64Const {
        value: i64,
    },
    F32Const {
        value: f32,
    },
    F64Const {
        value: f64,
    },
    I64ExtendI32U,
    I32WrapI64,

    // i32 maths
    I32Add,
    I32Mul,
    I32DivU,
    I32Sub,
    I32ShrU,
    I32Shl,
    I32And,
    I32Xor,

    // i32 comparators
    I32GtS,
    I32LtU,
    I32GtU,
    I32LtS,
    I32LeU,
    I32Ne,
    I32GeU,
    I32Eqz,
    I32Eq,
    I32Or,

    // i64 maths
    I64Add,
    I64Mul,
    I64Sub,
    I64ShrU,
    I64Shl,
    I64And,
    I64Or,

    // i64 comparators
    I64GtS,
    I64LtU,
    I64GtU,
    I64LeU,
    I64GeU,
    I64Eq,
    I64Ne,
    I64Eqz,

    // Placeholder for operators without an implementation; traps only if executed
    Unsupported {
        description: Box<String>,
    },
}
