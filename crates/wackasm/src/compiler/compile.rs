use anyhow::anyhow;
use wasmparser::{
    FuncValidator, FuncValidatorAllocations, FunctionBody, Operator, OperatorsReader, ValType,
    ValidatorResources,
};

use crate::{
    HaltError, Value,
    blocks::{BlockManager, lookahead_find_blocks},
    compiler::{
        RuntimeWackasmOperator, WackasmFunctionBody, WackasmOperator, block::BlockPass,
        register::RegStack,
    },
    functions::FunctionManager,
    stack::{val_type_to_size, val_types_to_size},
};

pub(crate) struct Compiler {
    pub(crate) func_body: WackasmFunctionBody,
    pub(crate) block_manager: BlockManager,
    pub(crate) func_validator: FuncValidator<ValidatorResources>,
    pub(crate) registers: RegStack,
    pub(crate) locals_slot_to_index: Vec<usize>,
    pub(crate) last_producer: Option<usize>,
    /// Code position of the most recent branch landing site.
    pub(crate) last_label: usize,
    transform_pc_lookup: Vec<usize>,
    branch_fixups: Vec<(usize, usize)>,
    labels: Vec<bool>,
}

impl Compiler {
    pub fn new(func_validator: FuncValidator<ValidatorResources>) -> Self {
        Self {
            func_body: WackasmFunctionBody::empty(),
            block_manager: BlockManager::new(),
            func_validator,
            registers: RegStack::new(0),
            locals_slot_to_index: Vec::new(),
            last_producer: None,
            last_label: 0,
            transform_pc_lookup: Vec::new(),
            branch_fixups: Vec::new(),
            labels: Vec::new(),
        }
    }

    pub fn create_locals(
        &mut self,
        params: &[ValType],
        local_types: Vec<ValType>,
    ) -> Result<(), HaltError> {
        self.func_body.params_slots = val_types_to_size(params);
        let locals_len = val_types_to_size(&local_types);
        let total = self
            .func_body
            .params_slots
            .checked_add(locals_len)
            .ok_or_else(|| anyhow!("local slot count overflow"))?;
        // The one-past-the-end register is used by zero-argument calls and empty returns.
        u16::try_from(total).map_err(|_| anyhow!("too many local register slots"))?;
        let mut slot_idx = 0;
        for ty in params {
            self.locals_slot_to_index.push(slot_idx);
            slot_idx += val_type_to_size(ty);
        }
        let mut locals = vec![Value(0); locals_len].into_boxed_slice();
        for ty in local_types {
            self.locals_slot_to_index.push(slot_idx);
            if matches!(ty, ValType::Ref(_)) {
                locals[slot_idx - self.func_body.params_slots] = Value::NULL_REF;
            }
            slot_idx += val_type_to_size(&ty);
        }
        self.func_body.local_defaults = locals;
        self.registers = RegStack::new(total);
        Ok(())
    }

    pub fn block_lookahead(&mut self, code: &FunctionBody) -> Result<(), HaltError> {
        self.block_manager = lookahead_find_blocks(code)?;
        let count = code.get_operators_reader()?.into_iter().count();
        self.labels = vec![false; count + 1];
        for block in self.block_manager.blocks.iter().flatten() {
            use crate::blocks::Block;
            match block {
                Block::Loop { start, end, .. } => {
                    self.labels[*start] = true;
                    self.labels[*end + 1] = true;
                }
                Block::Block { end, .. } => self.labels[*end + 1] = true,
                Block::If { r#else, end, .. } => {
                    self.labels[r#else.unwrap_or(*end) + 1] = true;
                    self.labels[*end + 1] = true;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn emit(&mut self, op: WackasmOperator) -> usize {
        let code = self.func_body.inner.as_mut().unwrap();
        let pc = code.len();
        code.push(op);
        pc
    }

    pub(crate) fn jump(&mut self, target: usize) {
        let pc = self.emit(WackasmOperator::Jump { offset: 0 });
        self.branch_fixups.push((pc, target));
    }

    pub(crate) fn fixup(&mut self, pc: usize, target: usize) {
        self.branch_fixups.push((pc, target));
    }

    /// Rewrites `br_if exit; br loop` as `br_unless loop; br exit`, so each iteration takes
    /// one branch. `pc` is the wasm pc of the `br`; nothing may branch to it.
    pub(crate) fn invert_into_backedge(&mut self, pc: usize, target: usize) -> bool {
        let jump_pc = self.code_len();
        if self.labels[pc] {
            return false;
        }
        let Some(last) = self
            .branch_fixups
            .last_mut()
            .filter(|(branch_pc, _)| *branch_pc + 1 == jump_pc)
        else {
            return false;
        };
        use WackasmOperator::*;
        let conditional = &mut self.func_body.inner.as_mut().unwrap()[last.0];
        *conditional = match conditional.clone() {
            JumpIfNez { cond, offset } => JumpIfEqz { cond, offset },
            JumpIfEqz { cond, offset } => JumpIfNez { cond, offset },
            JumpIfI32Cmp { lhs, rhs, kind, invert, offset } => JumpIfI32Cmp { lhs, rhs, kind, invert: !invert, offset },
            JumpIfI32CmpImm { lhs, imm, kind, invert, offset } => JumpIfI32CmpImm { lhs, imm, kind, invert: !invert, offset },
            JumpIfI64Cmp { lhs, rhs, kind, invert, offset } => JumpIfI64Cmp { lhs, rhs, kind, invert: !invert, offset },
            JumpIfI64CmpImm { lhs, imm, kind, invert, offset } => JumpIfI64CmpImm { lhs, imm, kind, invert: !invert, offset },
            I32AddImmJumpIfNez { dst, lhs, imm, offset } => I32AddImmJumpIfEqz { dst, lhs, imm, offset },
            I32AddImmJumpIfEqz { dst, lhs, imm, offset } => I32AddImmJumpIfNez { dst, lhs, imm, offset },
            _ => return false,
        };
        let exit = std::mem::replace(&mut last.1, target);
        self.jump(exit);
        true
    }

    pub(crate) fn code_len(&self) -> usize {
        self.func_body.inner.as_ref().unwrap().len()
    }

    pub(crate) fn mark_loop_header(&mut self, old_pc: usize) {
        // Entry providers are initialised once; backedges already carry canonical parameters.
        self.transform_pc_lookup[old_pc] = self.code_len();
        // Backedges land just after the header's ConsumeFuel.
        self.last_label = self.code_len() + 1;
    }

    pub(crate) fn patch_relative(&mut self, pc: usize, target: usize) -> Result<(), HaltError> {
        let offset = i32::try_from(target as i64 - pc as i64)
            .map_err(|_| anyhow!("branch offset exceeds i32"))?;
        match &mut self.func_body.inner.as_mut().unwrap()[pc] {
            WackasmOperator::Jump { offset: value }
            | WackasmOperator::JumpIfNez { offset: value, .. }
            | WackasmOperator::JumpIfEqz { offset: value, .. }
            | WackasmOperator::JumpIfI32Cmp { offset: value, .. }
            | WackasmOperator::JumpIfI32CmpImm { offset: value, .. }
            | WackasmOperator::JumpIfI64Cmp { offset: value, .. }
            | WackasmOperator::JumpIfI64CmpImm { offset: value, .. }
            | WackasmOperator::I32AddImmJumpIfNez { offset: value, .. }
            | WackasmOperator::I32AddImmJumpIfEqz { offset: value, .. } => *value = offset,
            _ => return Err(anyhow!("branch fixup points at non-branch").into()),
        }
        Ok(())
    }

    pub(crate) fn validator_slots(&self) -> usize {
        let height = self.func_validator.operand_stack_height() as usize;
        (0..height)
            .map(|depth| {
                if self.func_validator.get_operand_type(depth).flatten() == Some(ValType::V128) {
                    2
                } else {
                    1
                }
            })
            .sum()
    }

    fn reconcile(&mut self) -> Result<(), HaltError> {
        self.registers.clear();
        let height = self.func_validator.operand_stack_height() as usize;
        for depth in (0..height).rev() {
            let width =
                if self.func_validator.get_operand_type(depth).flatten() == Some(ValType::V128) {
                    2
                } else {
                    1
                };
            self.registers.push_temp(width)?;
        }
        self.last_producer = None;
        Ok(())
    }

    pub fn transform(
        &mut self,
        code: &FunctionBody,
        functions: &FunctionManager,
    ) -> Result<(), HaltError> {
        let mut reader = code.get_binary_reader();
        self.func_validator.read_locals(&mut reader)?;
        let mut ops = OperatorsReader::new(reader);
        let mut blocks = BlockPass::new();
        let mut pc = 0;
        while !ops.eof() {
            if self.labels[pc] {
                self.last_producer = None;
                self.last_label = self.code_len();
            }
            self.transform_pc_lookup.push(self.code_len());
            let (operator, offset) = ops.read_with_offset()?;
            let unreachable = self
                .func_validator
                .get_control_frame(0)
                .is_some_and(|frame| frame.unreachable);
            let structural = matches!(
                operator,
                Operator::Block { .. }
                    | Operator::Loop { .. }
                    | Operator::If { .. }
                    | Operator::Else
                    | Operator::End
            );
            let mut unsupported = false;
            if structural || !unreachable {
                if !blocks.compile(self, functions, pc, &operator, unreachable)? {
                    unsupported = !self.compile_register(functions, &operator)?;
                }
            }
            self.func_validator.op(offset, &operator)?;
            let now_unreachable = self
                .func_validator
                .get_control_frame(0)
                .is_some_and(|frame| frame.unreachable);
            if unsupported
                || unreachable
                || now_unreachable
                || matches!(operator, Operator::Else | Operator::End)
            {
                self.reconcile()?;
            }
            debug_assert_eq!(
                self.registers.height_slots,
                self.validator_slots(),
                "register stack disagrees with validator after {operator:?}"
            );
            self.func_body.max_height_slots = self
                .func_body
                .max_height_slots
                .max(u32::try_from(self.registers.max_height_slots)?);
            pc += 1;
        }
        self.transform_pc_lookup.push(self.code_len());
        ops.get_binary_reader()
            .finish_expression(&self.func_validator.visitor(ops.original_position()))?;
        Ok(())
    }

    pub fn target_rewrite(&mut self) -> Result<(), HaltError> {
        for (pc, old_target) in std::mem::take(&mut self.branch_fixups) {
            let mut target = *self
                .transform_pc_lookup
                .get(old_target)
                .ok_or_else(|| anyhow!("no new pc for old pc {old_target}"))?;
            if target <= pc {
                // Backward branches meter fuel themselves, so they skip the loop header's charge.
                if !matches!(
                    self.func_body.inner.as_ref().unwrap().get(target),
                    Some(WackasmOperator::ConsumeFuel)
                ) {
                    return Err(anyhow!("backward branch does not target a loop header").into());
                }
                target += 1;
            }
            self.patch_relative(pc, target)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(WackasmFunctionBody, FuncValidatorAllocations), HaltError> {
        self.func_body.runtime = self
            .func_body
            .inner
            .take()
            .unwrap()
            .into_iter()
            .map(RuntimeWackasmOperator::from)
            .collect();
        Ok((self.func_body, self.func_validator.into_allocations()))
    }
}
