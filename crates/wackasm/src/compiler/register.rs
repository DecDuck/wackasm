use anyhow::anyhow;
use wasmparser::{Operator, ValType, WasmModuleResources as _};

use crate::{
    HaltError,
    compiler::{I32Comparison, Reg, WackasmOperator, compile::Compiler},
    functions::FunctionManager,
    module::FunctionIndex,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Provider {
    Local(Reg),
    Const(u64),
    Temp(Reg),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Operand {
    pub(crate) provider: Provider,
    pub(crate) reg: Reg,
    pub(crate) width: usize,
}

pub(crate) struct RegStack {
    pub(crate) providers: Vec<Operand>,
    pub(crate) locals_len: usize,
    pub(crate) height_slots: usize,
    pub(crate) max_height_slots: usize,
    local_live: Vec<usize>,
}

impl RegStack {
    pub(crate) fn new(locals_len: usize) -> Self {
        Self {
            providers: Vec::new(),
            locals_len,
            height_slots: 0,
            max_height_slots: 0,
            local_live: vec![0; locals_len],
        }
    }

    pub(crate) fn reg_at(&self, height: usize) -> Result<Reg, HaltError> {
        Ok(Reg::try_from(
            self.locals_len
                .checked_add(height)
                .ok_or_else(|| anyhow!("register index overflow"))?,
        )
        .map_err(|_| anyhow!("register index exceeds u16"))?)
    }

    pub(crate) fn push(&mut self, provider: Provider, width: usize) -> Result<Reg, HaltError> {
        let reg = self.reg_at(self.height_slots)?;
        let new_height = self
            .height_slots
            .checked_add(width)
            .ok_or_else(|| anyhow!("operand register count overflow"))?;
        self.reg_at(new_height)?;
        if let Provider::Local(local) = provider {
            self.local_live[local as usize] += 1;
        }
        self.providers.push(Operand {
            provider,
            reg,
            width,
        });
        self.height_slots = new_height;
        self.max_height_slots = self.max_height_slots.max(new_height);
        Ok(reg)
    }

    pub(crate) fn push_temp(&mut self, width: usize) -> Result<Reg, HaltError> {
        let reg = self.reg_at(self.height_slots)?;
        self.push(Provider::Temp(reg), width)
    }

    pub(crate) fn pop(&mut self) -> Result<Operand, HaltError> {
        let value = self
            .providers
            .pop()
            .ok_or_else(|| anyhow!("empty register stack"))?;
        self.height_slots -= value.width;
        if let Provider::Local(local) = value.provider {
            self.local_live[local as usize] -= 1;
        }
        Ok(value)
    }

    pub(crate) fn clear(&mut self) {
        self.providers.clear();
        self.height_slots = 0;
        self.local_live.fill(0);
    }
}

impl Compiler {
    pub(crate) fn copy(&mut self, dst: Reg, src: Reg, width: usize) {
        if dst != src {
            self.emit(if width == 2 {
                WackasmOperator::CopyWide { dst, src }
            } else {
                WackasmOperator::Copy { dst, src }
            });
        }
    }

    pub(crate) fn materialize_operand(&mut self, value: Operand) -> Reg {
        match value.provider {
            Provider::Local(local) | Provider::Temp(local) => local,
            Provider::Const(bits) => {
                // Both i32/f32 and i64/f64 occupy one Value, not two.
                self.emit(if bits >> 32 == 0 {
                    WackasmOperator::CopyImm32 {
                        dst: value.reg,
                        value: bits as u32,
                    }
                } else {
                    WackasmOperator::CopyImm64 {
                        dst: value.reg,
                        value: [bits as u32, (bits >> 32) as u32],
                    }
                });
                value.reg
            }
        }
    }

    fn materialize_stack_value(&mut self, index: usize) {
        let value = self.registers.providers[index];
        let src = self.materialize_operand(value);
        self.copy(value.reg, src, value.width);
        if let Provider::Local(local) = value.provider {
            self.registers.local_live[local as usize] -= 1;
        }
        self.registers.providers[index].provider = Provider::Temp(value.reg);
    }

    pub(crate) fn materialize_all(&mut self) {
        for index in 0..self.registers.providers.len() {
            self.materialize_stack_value(index);
        }
    }

    fn preserve_local(&mut self, local: Reg) {
        if self.registers.local_live[local as usize] == 0 {
            return;
        }
        for index in 0..self.registers.providers.len() {
            if self.registers.providers[index].provider == Provider::Local(local) {
                self.materialize_stack_value(index);
            }
        }
    }

    fn produce(&mut self, operator: WackasmOperator, width: usize) -> Result<(), HaltError> {
        self.last_producer = Some(self.emit(operator));
        self.registers.push_temp(width)?;
        Ok(())
    }

    fn retarget(&mut self, from: Reg, to: Reg) -> bool {
        let Some(pc) = self.last_producer.filter(|pc| *pc + 1 == self.code_len()) else {
            return false;
        };
        let operator = &mut self.func_body.inner.as_mut().unwrap()[pc];
        use WackasmOperator::*;
        let dst = match operator {
            Copy { dst, .. }
            | CopyWide { dst, .. }
            | CopyImm32 { dst, .. }
            | CopyImm64 { dst, .. }
            | Select { dst, .. }
            | SelectWide { dst, .. }
            | GlobalGet { dst, .. }
            | GlobalGetWide { dst, .. }
            | MemoryGrow { dst, .. }
            | I32Load { dst, .. }
            | I32Load8U { dst, .. }
            | I32Load8S { dst, .. }
            | I32Load16U { dst, .. }
            | I32Load16S { dst, .. }
            | I64Load { dst, .. }
            | I64Load8U { dst, .. }
            | I64Load8S { dst, .. }
            | I64Load16U { dst, .. }
            | I64Load16S { dst, .. }
            | I64Load32U { dst, .. }
            | I64Load32S { dst, .. }
            | F32Load { dst, .. }
            | F64Load { dst, .. }
            | V128Load { dst, .. }
            | I32Extend8S { dst, .. }
            | I64ExtendI32U { dst, .. }
            | I32WrapI64 { dst, .. }
            | I32Eqz { dst, .. }
            | I64Eqz { dst, .. }
            | I32Add { dst, .. }
            | I32Mul { dst, .. }
            | I32DivU { dst, .. }
            | I32Sub { dst, .. }
            | I32ShrU { dst, .. }
            | I32Shl { dst, .. }
            | I32And { dst, .. }
            | I32Xor { dst, .. }
            | I32Rotr { dst, .. }
            | I32GtS { dst, .. }
            | I32LtU { dst, .. }
            | I32GtU { dst, .. }
            | I32LtS { dst, .. }
            | I32GeS { dst, .. }
            | I32LeU { dst, .. }
            | I32Ne { dst, .. }
            | I32GeU { dst, .. }
            | I32Eq { dst, .. }
            | I32Or { dst, .. }
            | I64Add { dst, .. }
            | I64Mul { dst, .. }
            | I64Sub { dst, .. }
            | I64ShrU { dst, .. }
            | I64Shl { dst, .. }
            | I64And { dst, .. }
            | I64Or { dst, .. }
            | I64GtS { dst, .. }
            | I64LtU { dst, .. }
            | I64GtU { dst, .. }
            | I64LeU { dst, .. }
            | I64GeU { dst, .. }
            | I64Eq { dst, .. }
            | I64Ne { dst, .. }
            | I32AddImm { dst, .. }
            | I32SubImm { dst, .. }
            | I32AndImm { dst, .. }
            | I32ShlImm { dst, .. }
            | I32ShrUImm { dst, .. }
            | I64AddImm { dst, .. }
            | I32EqImm { dst, .. }
            | I32NeImm { dst, .. }
            | I32LtSImm { dst, .. }
            | I32LtUImm { dst, .. }
            | I32GtSImm { dst, .. }
            | I32GtUImm { dst, .. }
            | I32GeSImm { dst, .. }
            | I32GeUImm { dst, .. }
            | I32LeUImm { dst, .. } => dst,
            _ => return false,
        };
        if *dst != from {
            return false;
        }
        *dst = to;
        true
    }

    pub(crate) fn conditional(&mut self, cond: Reg, invert: bool) -> usize {
        use WackasmOperator::*;
        if let Some(pc) = self.last_producer.filter(|pc| *pc + 1 == self.code_len()) {
            let previous = &self.func_body.inner.as_ref().unwrap()[pc];
            let replacement = match *previous {
                I32Eqz { dst, src } if dst == cond => Some(if invert {
                    JumpIfNez {
                        cond: src,
                        offset: 0,
                    }
                } else {
                    JumpIfEqz {
                        cond: src,
                        offset: 0,
                    }
                }),
                I32Eq { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::Eq,
                    invert,
                    offset: 0,
                }),
                I32Ne { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::Ne,
                    invert,
                    offset: 0,
                }),
                I32LtS { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::LtS,
                    invert,
                    offset: 0,
                }),
                I32LtU { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::LtU,
                    invert,
                    offset: 0,
                }),
                I32GtS { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::GtS,
                    invert,
                    offset: 0,
                }),
                I32GtU { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::GtU,
                    invert,
                    offset: 0,
                }),
                I32GeS { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::GeS,
                    invert,
                    offset: 0,
                }),
                I32GeU { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::GeU,
                    invert,
                    offset: 0,
                }),
                I32LeU { dst, lhs, rhs } if dst == cond => Some(JumpIfI32Cmp {
                    lhs,
                    rhs,
                    kind: I32Comparison::LeU,
                    invert,
                    offset: 0,
                }),
                _ => None,
            };
            if let Some(replacement) = replacement {
                self.func_body.inner.as_mut().unwrap()[pc] = replacement;
                self.last_producer = None;
                return pc;
            }
        }
        self.last_producer = None;
        self.emit(if invert {
            JumpIfEqz { cond, offset: 0 }
        } else {
            JumpIfNez { cond, offset: 0 }
        })
    }

    pub(crate) fn compile_register(
        &mut self,
        functions: &FunctionManager,
        op: &Operator<'_>,
    ) -> Result<bool, HaltError> {
        macro_rules! unary {
            ($name:ident) => {{
                let value = self.registers.pop()?;
                let src = self.materialize_operand(value);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                self.produce(WackasmOperator::$name { dst, src }, 1)?;
            }};
        }
        macro_rules! binary {
            ($name:ident) => {{
                let right = self.registers.pop()?;
                let left = self.registers.pop()?;
                let lhs = self.materialize_operand(left);
                let rhs = self.materialize_operand(right);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                self.produce(WackasmOperator::$name { dst, lhs, rhs }, 1)?;
            }};
        }
        macro_rules! immediate {
            ($name:ident, $imm_name:ident) => {{
                immediate!($name, $imm_name, false);
            }};
            ($name:ident, $imm_name:ident, $commutative:expr) => {{
                let mut right = self.registers.pop()?;
                let mut left = self.registers.pop()?;
                if $commutative
                    && matches!(left.provider, Provider::Const(_))
                    && !matches!(right.provider, Provider::Const(_))
                {
                    std::mem::swap(&mut left, &mut right);
                }
                let lhs = self.materialize_operand(left);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                let operator = if let Provider::Const(bits) = right.provider {
                    WackasmOperator::$imm_name {
                        dst,
                        lhs,
                        imm: bits as u32 as i32,
                    }
                } else {
                    let rhs = self.materialize_operand(right);
                    WackasmOperator::$name { dst, lhs, rhs }
                };
                self.produce(operator, 1)?;
            }};
        }
        macro_rules! load {
            ($name:ident, $memarg:ident, $width:expr) => {{
                let value = self.registers.pop()?;
                let addr = self.materialize_operand(value);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                self.produce(
                    WackasmOperator::$name {
                        dst,
                        addr,
                        memarg: (*$memarg).try_into()?,
                    },
                    $width,
                )?;
            }};
        }
        macro_rules! store {
            ($name:ident, $memarg:ident) => {{
                let operand = self.registers.pop()?;
                let address = self.registers.pop()?;
                let value = self.materialize_operand(operand);
                let addr = self.materialize_operand(address);
                self.emit(WackasmOperator::$name {
                    addr,
                    value,
                    memarg: (*$memarg).try_into()?,
                });
                self.last_producer = None;
            }};
        }
        match op {
            Operator::Nop => {}
            Operator::Unreachable => {
                self.emit(WackasmOperator::Unreachable);
                self.last_producer = None;
            }
            Operator::I32Const { value } => {
                self.registers
                    .push(Provider::Const(*value as i64 as u64), 1)?;
            }
            Operator::I64Const { value } => {
                self.registers.push(Provider::Const(*value as u64), 1)?;
            }
            Operator::F32Const { value } => {
                self.registers
                    .push(Provider::Const(value.bits() as u64), 1)?;
            }
            Operator::F64Const { value } => {
                self.registers.push(Provider::Const(value.bits()), 1)?;
            }
            Operator::LocalGet { local_index } => {
                let local = Reg::try_from(
                    *self
                        .locals_slot_to_index
                        .get(*local_index as usize)
                        .ok_or_else(|| anyhow!("invalid local index"))?,
                )?;
                let width =
                    if self.func_validator.get_local_type(*local_index) == Some(ValType::V128) {
                        2
                    } else {
                        1
                    };
                self.registers.push(Provider::Local(local), width)?;
            }
            Operator::LocalSet { local_index } | Operator::LocalTee { local_index } => {
                let local = Reg::try_from(
                    *self
                        .locals_slot_to_index
                        .get(*local_index as usize)
                        .ok_or_else(|| anyhow!("invalid local index"))?,
                )?;
                // Preserve aliases before the write, including a top-of-stack local.get.
                self.preserve_local(local);
                let value = self.registers.pop()?;
                if !matches!(value.provider, Provider::Temp(reg) if self.retarget(reg, local)) {
                    let src = self.materialize_operand(value);
                    self.copy(local, src, value.width);
                }
                self.last_producer = None;
                if matches!(op, Operator::LocalTee { .. }) {
                    self.registers.push(Provider::Local(local), value.width)?;
                }
            }
            Operator::Drop => {
                self.registers.pop()?;
                self.last_producer = None;
            }
            Operator::GlobalGet { global_index } => {
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                let wide = self
                    .func_validator
                    .resources()
                    .global_at(*global_index)
                    .is_some_and(|ty| ty.content_type == ValType::V128);
                self.produce(
                    if wide {
                        WackasmOperator::GlobalGetWide {
                            dst,
                            global_index: *global_index,
                        }
                    } else {
                        WackasmOperator::GlobalGet {
                            dst,
                            global_index: *global_index,
                        }
                    },
                    if wide { 2 } else { 1 },
                )?;
            }
            Operator::GlobalSet { global_index } => {
                let value = self.registers.pop()?;
                let src = self.materialize_operand(value);
                self.emit(if value.width == 2 {
                    WackasmOperator::GlobalSetWide {
                        src,
                        global_index: *global_index,
                    }
                } else {
                    WackasmOperator::GlobalSet {
                        src,
                        global_index: *global_index,
                    }
                });
                self.last_producer = None;
            }
            Operator::Select => {
                let c = self.registers.pop()?;
                let second = self.registers.pop()?;
                let first = self.registers.pop()?;
                let cond = self.materialize_operand(c);
                let a = self.materialize_operand(first);
                let b = self.materialize_operand(second);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                self.produce(
                    if first.width == 2 {
                        WackasmOperator::SelectWide { dst, cond, a, b }
                    } else {
                        WackasmOperator::Select { dst, cond, a, b }
                    },
                    first.width,
                )?;
            }
            Operator::Call { function_index } => {
                self.materialize_all();
                let ty = functions.function_type_idx(&FunctionIndex(*function_index))?;
                for _ in ty.params() {
                    self.registers.pop()?;
                }
                let base = self.registers.reg_at(self.registers.height_slots)?;
                self.emit(WackasmOperator::Call {
                    function_index: *function_index,
                    base,
                });
                for result in ty.results() {
                    self.registers
                        .push_temp(if *result == ValType::V128 { 2 } else { 1 })?;
                }
                self.last_producer = None;
            }
            Operator::CallIndirect {
                table_index,
                type_index,
                ..
            } => {
                self.materialize_all();
                let index = self.registers.pop()?.reg;
                let ty = functions
                    .function_type_idx_type(&crate::module::FunctionTypeIndex(*type_index))?;
                for _ in ty.params() {
                    self.registers.pop()?;
                }
                let base = self.registers.reg_at(self.registers.height_slots)?;
                self.emit(WackasmOperator::CallIndirect {
                    index,
                    table_index: *table_index,
                    type_index: *type_index,
                    base,
                });
                for result in ty.results() {
                    self.registers
                        .push_temp(if *result == ValType::V128 { 2 } else { 1 })?;
                }
                self.last_producer = None;
            }
            Operator::MemoryGrow { mem } => {
                let value = self.registers.pop()?;
                let amount = self.materialize_operand(value);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                self.produce(
                    WackasmOperator::MemoryGrow {
                        dst,
                        amount,
                        mem: *mem,
                    },
                    1,
                )?;
            }
            Operator::MemoryCopy { dst_mem, src_mem } => {
                let length = self.registers.pop()?;
                let source = self.registers.pop()?;
                let dest = self.registers.pop()?;
                let len = self.materialize_operand(length);
                let src = self.materialize_operand(source);
                let dst = self.materialize_operand(dest);
                self.emit(WackasmOperator::MemoryCopy {
                    dst_mem: u16::try_from(*dst_mem)?,
                    src_mem: u16::try_from(*src_mem)?,
                    dst,
                    src,
                    len,
                });
                self.last_producer = None;
            }
            Operator::MemoryFill { mem } => {
                let length = self.registers.pop()?;
                let val = self.registers.pop()?;
                let dest = self.registers.pop()?;
                let len = self.materialize_operand(length);
                let value = self.materialize_operand(val);
                let addr = self.materialize_operand(dest);
                self.emit(WackasmOperator::MemoryFill {
                    mem: *mem,
                    addr,
                    value,
                    len,
                });
                self.last_producer = None;
            }
            Operator::I32Load { memarg } => load!(I32Load, memarg, 1),
            Operator::I32Load8U { memarg } => load!(I32Load8U, memarg, 1),
            Operator::I32Load8S { memarg } => load!(I32Load8S, memarg, 1),
            Operator::I32Load16U { memarg } => load!(I32Load16U, memarg, 1),
            Operator::I32Load16S { memarg } => load!(I32Load16S, memarg, 1),
            Operator::I64Load { memarg } => load!(I64Load, memarg, 1),
            Operator::I64Load8U { memarg } => load!(I64Load8U, memarg, 1),
            Operator::I64Load8S { memarg } => load!(I64Load8S, memarg, 1),
            Operator::I64Load16U { memarg } => load!(I64Load16U, memarg, 1),
            Operator::I64Load16S { memarg } => load!(I64Load16S, memarg, 1),
            Operator::I64Load32U { memarg } => load!(I64Load32U, memarg, 1),
            Operator::I64Load32S { memarg } => load!(I64Load32S, memarg, 1),
            Operator::F32Load { memarg } => load!(F32Load, memarg, 1),
            Operator::F64Load { memarg } => load!(F64Load, memarg, 1),
            Operator::V128Load { memarg } => load!(V128Load, memarg, 2),
            Operator::I32Store { memarg } => store!(I32Store, memarg),
            Operator::I32Store8 { memarg } => store!(I32Store8, memarg),
            Operator::I32Store16 { memarg } => store!(I32Store16, memarg),
            Operator::I64Store { memarg } => store!(I64Store, memarg),
            Operator::I64Store8 { memarg } => store!(I64Store8, memarg),
            Operator::I64Store16 { memarg } => store!(I64Store16, memarg),
            Operator::I64Store32 { memarg } => store!(I64Store32, memarg),
            Operator::F32Store { memarg } => store!(F32Store, memarg),
            Operator::F64Store { memarg } => store!(F64Store, memarg),
            Operator::V128Store { memarg } => store!(V128Store, memarg),
            Operator::I32Extend8S => unary!(I32Extend8S),
            Operator::I64ExtendI32U => unary!(I64ExtendI32U),
            Operator::I32WrapI64 => unary!(I32WrapI64),
            Operator::I32Eqz => unary!(I32Eqz),
            Operator::I64Eqz => unary!(I64Eqz),
            Operator::I32Add => immediate!(I32Add, I32AddImm, true),
            Operator::I32Sub => immediate!(I32Sub, I32SubImm),
            Operator::I32ShrU => immediate!(I32ShrU, I32ShrUImm),
            Operator::I32Shl => immediate!(I32Shl, I32ShlImm),
            Operator::I32And => immediate!(I32And, I32AndImm, true),
            Operator::I32GtS => immediate!(I32GtS, I32GtSImm),
            Operator::I32LtS => immediate!(I32LtS, I32LtSImm),
            Operator::I32LtU => immediate!(I32LtU, I32LtUImm),
            Operator::I32GtU => immediate!(I32GtU, I32GtUImm),
            Operator::I32GeS => immediate!(I32GeS, I32GeSImm),
            Operator::I32LeU => immediate!(I32LeU, I32LeUImm),
            Operator::I32Ne => immediate!(I32Ne, I32NeImm),
            Operator::I32GeU => immediate!(I32GeU, I32GeUImm),
            Operator::I32Eq => immediate!(I32Eq, I32EqImm),
            Operator::I32Mul => binary!(I32Mul),
            Operator::I32DivU => binary!(I32DivU),
            Operator::I32Xor => binary!(I32Xor),
            Operator::I32Rotr => binary!(I32Rotr),
            Operator::I32Or => binary!(I32Or),
            Operator::I64Add => {
                let mut right = self.registers.pop()?;
                let mut left = self.registers.pop()?;
                if matches!(left.provider, Provider::Const(bits) if i32::try_from(bits as i64).is_ok())
                    && !matches!(right.provider, Provider::Const(_))
                {
                    std::mem::swap(&mut left, &mut right);
                }
                let lhs = self.materialize_operand(left);
                let dst = self.registers.reg_at(self.registers.height_slots)?;
                let operator = match right.provider {
                    Provider::Const(bits) if i32::try_from(bits as i64).is_ok() => {
                        WackasmOperator::I64AddImm {
                            dst,
                            lhs,
                            imm: bits as i64 as i32,
                        }
                    }
                    _ => {
                        let rhs = self.materialize_operand(right);
                        WackasmOperator::I64Add { dst, lhs, rhs }
                    }
                };
                self.produce(operator, 1)?;
            }
            Operator::I64Mul => binary!(I64Mul),
            Operator::I64Sub => binary!(I64Sub),
            Operator::I64ShrU => binary!(I64ShrU),
            Operator::I64Shl => binary!(I64Shl),
            Operator::I64And => binary!(I64And),
            Operator::I64Or => binary!(I64Or),
            Operator::I64GtS => binary!(I64GtS),
            Operator::I64LtU => binary!(I64LtU),
            Operator::I64GtU => binary!(I64GtU),
            Operator::I64LeU => binary!(I64LeU),
            Operator::I64GeU => binary!(I64GeU),
            Operator::I64Eq => binary!(I64Eq),
            Operator::I64Ne => binary!(I64Ne),
            _ => {
                self.materialize_all();
                self.emit(WackasmOperator::Unsupported {
                    description: Box::new(format!("{op:?}")),
                });
                self.last_producer = None;
                return Ok(false);
            }
        }
        Ok(true)
    }
}
