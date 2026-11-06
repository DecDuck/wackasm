use anyhow::anyhow;
use wasmparser::Operator;

use crate::{
    HaltError,
    blocks::Block,
    compiler::{Reg, WackasmOperator, compile::Compiler},
    functions::FunctionManager,
    module::BlockIndex,
    stack::val_types_to_size,
};

struct OpenBlock {
    index: BlockIndex,
    entry_height_slots: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Branch {
    Return {
        src: Reg,
        len: Reg,
    },
    Target {
        target: usize,
        dst: Reg,
        src: Reg,
        len: Reg,
    },
}

pub(crate) struct BlockPass {
    blocks: Vec<OpenBlock>,
}

impl BlockPass {
    pub(crate) fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    fn resolve(
        &self,
        compiler: &Compiler,
        functions: &FunctionManager,
        depth: u32,
    ) -> Result<Branch, HaltError> {
        if depth as usize >= self.blocks.len() {
            let frame = compiler
                .func_validator
                .get_control_frame(self.blocks.len())
                .ok_or_else(|| anyhow!("missing function control frame"))?;
            let result_slots = match frame.block_type {
                wasmparser::BlockType::Empty => 0,
                wasmparser::BlockType::Type(ty) => crate::stack::val_type_to_size(&ty),
                wasmparser::BlockType::FuncType(index) => val_types_to_size(
                    functions
                        .function_type_idx_type(&crate::module::FunctionTypeIndex(index))?
                        .results(),
                ),
            };
            let len = Reg::try_from(result_slots)?;
            let src = compiler.registers.reg_at(
                compiler
                    .registers
                    .height_slots
                    .checked_sub(len as usize)
                    .ok_or_else(|| anyhow!("return stack underflow"))?,
            )?;
            return Ok(Branch::Return { src, len });
        }
        let open = &self.blocks[self.blocks.len() - 1 - depth as usize];
        let block = compiler.block_manager.get(&open.index)?;
        let (target, types) = match block {
            Block::Loop { start, .. } => (*start, block.call_params(functions)?),
            Block::Block { end, .. } | Block::If { end, .. } => {
                (*end + 1, block.return_params(functions)?)
            }
        };
        let len = Reg::try_from(types.map(val_types_to_size).unwrap_or(0))?;
        let src = compiler.registers.reg_at(
            compiler
                .registers
                .height_slots
                .checked_sub(len as usize)
                .ok_or_else(|| anyhow!("branch stack underflow"))?,
        )?;
        let dst = compiler.registers.reg_at(open.entry_height_slots)?;
        Ok(Branch::Target {
            target,
            dst,
            src,
            len,
        })
    }

    fn needs_copy(branch: Branch) -> bool {
        matches!(branch, Branch::Target { dst, src, len, .. } if len != 0 && dst != src)
    }

    fn emit_branch(compiler: &mut Compiler, branch: Branch) {
        match branch {
            Branch::Return { src, len } => {
                compiler.emit(WackasmOperator::Return { src, len });
            }
            Branch::Target {
                target,
                dst,
                src,
                len,
            } => {
                if len != 0 && dst != src {
                    compiler.emit(WackasmOperator::CopySpan { dst, src, len });
                }
                compiler.jump(target);
            }
        }
    }

    pub(crate) fn compile(
        &mut self,
        compiler: &mut Compiler,
        functions: &FunctionManager,
        pc: usize,
        op: &Operator<'_>,
        unreachable: bool,
    ) -> Result<bool, HaltError> {
        match op {
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                let index = BlockIndex(u32::try_from(pc)?);
                let block = *compiler.block_manager.get(&index)?;
                let params = block
                    .call_params(functions)?
                    .map(val_types_to_size)
                    .unwrap_or(0);
                if !unreachable {
                    let cond = if matches!(op, Operator::If { .. }) {
                        let value = compiler.registers.pop()?;
                        Some(compiler.materialize_operand(value))
                    } else {
                        None
                    };
                    compiler.materialize_all();
                    if let Some(cond) = cond {
                        let (_, r#else, end) = block.if_block()?;
                        let branch_pc = compiler.conditional(cond, true);
                        compiler.fixup(branch_pc, r#else.unwrap_or(end) + 1);
                    }
                    if matches!(op, Operator::Loop { .. }) {
                        compiler.mark_loop_header(pc);
                        compiler.emit(WackasmOperator::ConsumeFuel);
                    }
                }
                let entry_height_slots = compiler.registers.height_slots.saturating_sub(params);
                self.blocks.push(OpenBlock {
                    index,
                    entry_height_slots,
                });
            }
            Operator::Else => {
                if !unreachable {
                    compiler.materialize_all();
                    let open = self
                        .blocks
                        .last()
                        .ok_or_else(|| anyhow!("else without if"))?;
                    let (_, _, end) = compiler.block_manager.get(&open.index)?.if_block()?;
                    compiler.jump(end + 1);
                }
            }
            Operator::End => {
                if !unreachable {
                    compiler.materialize_all();
                    if self.blocks.is_empty() {
                        let branch = self.resolve(compiler, functions, 0)?;
                        Self::emit_branch(compiler, branch);
                    }
                }
                self.blocks.pop();
            }
            Operator::Return => {
                compiler.materialize_all();
                let branch = self.resolve(compiler, functions, self.blocks.len() as u32)?;
                Self::emit_branch(compiler, branch);
            }
            Operator::Br { relative_depth } => {
                compiler.materialize_all();
                let branch = self.resolve(compiler, functions, *relative_depth)?;
                match branch {
                    Branch::Target { target, .. }
                        if target < pc
                            && !Self::needs_copy(branch)
                            && compiler.invert_into_backedge(pc, target) => {}
                    _ => Self::emit_branch(compiler, branch),
                }
            }
            Operator::BrIf { relative_depth } => {
                let value = compiler.registers.pop()?;
                let cond = compiler.materialize_operand(value);
                compiler.materialize_all();
                let branch = self.resolve(compiler, functions, *relative_depth)?;
                match branch {
                    Branch::Return { src, len } => {
                        compiler.emit(WackasmOperator::ReturnIfNez { cond, src, len });
                    }
                    Branch::Target { target, .. } if !Self::needs_copy(branch) => {
                        let branch_pc = compiler.conditional(cond, false);
                        compiler.fixup(branch_pc, target);
                    }
                    _ => {
                        let skip = compiler.conditional(cond, true);
                        Self::emit_branch(compiler, branch);
                        compiler.patch_relative(skip, compiler.code_len())?;
                    }
                }
            }
            Operator::BrTable { targets } => {
                let value = compiler.registers.pop()?;
                let index = compiler.materialize_operand(value);
                compiler.materialize_all();
                let mut branches = targets
                    .targets()
                    .map(|target| self.resolve(compiler, functions, target?))
                    .collect::<Result<Vec<_>, HaltError>>()?;
                branches.push(self.resolve(compiler, functions, targets.default())?);
                let len = u32::try_from(branches.len())?;
                compiler.emit(WackasmOperator::BrTable { index, len });
                let table_start = compiler.code_len();
                for branch in &branches {
                    let entry_pc = compiler.emit(WackasmOperator::Jump { offset: 0 });
                    if let Branch::Target { target, .. } = *branch
                        && !Self::needs_copy(*branch)
                    {
                        compiler.fixup(entry_pc, target);
                    }
                }
                let mut trampolines = std::collections::HashMap::new();
                for (entry, branch) in branches.into_iter().enumerate() {
                    if matches!(branch, Branch::Return { .. }) || Self::needs_copy(branch) {
                        let trampoline = *trampolines.entry(branch).or_insert_with(|| {
                            let trampoline = compiler.code_len();
                            Self::emit_branch(compiler, branch);
                            trampoline
                        });
                        compiler.patch_relative(table_start + entry, trampoline)?;
                    }
                }
            }
            _ => return Ok(false),
        }
        compiler.last_producer = None;
        Ok(true)
    }
}
