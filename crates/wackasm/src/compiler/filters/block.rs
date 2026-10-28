use anyhow::anyhow;
use wasmparser::{FuncValidator, Operator, ValType};

use crate::{
    HaltError,
    blocks::Block,
    compiler::{
        BlockParams, DropKeep, WackasmOperator, WackasmTableTarget, WackasmTableTargets,
        compile::Compiler, filter::CompilerFilter,
    },
    functions::FunctionManager,
    macros::trap,
    module::BlockIndex,
    stack::val_types_to_size,
};

pub struct BlockPass {
    block_stack: Vec<BlockIndex>,
}

pub enum BranchResolveResult {
    Unreachable,
    Return,
    Target { target: u32, dk: DropKeep },
}

impl BlockPass {
    fn resolve_branch(
        &self,
        compiler: &Compiler,
        functions: &FunctionManager,
        depth: u32,
        stack_pops: u32,
    ) -> Result<BranchResolveResult, HaltError> {
        let validator = &compiler.func_validator;

        // If it's unreachable
        if validator
            .get_control_frame(0)
            .ok_or_else(|| anyhow!("no control frame while resolving branch"))?
            .unreachable
        {
            return Ok(BranchResolveResult::Unreachable);
        }

        // If it's a function return
        if depth >= self.block_stack.len() as u32 {
            return Ok(BranchResolveResult::Return);
        }

        // Branch to a block
        let target_block = self.block_stack[self.block_stack.len() - 1 - depth as usize];
        let block = compiler.block_manager.get(&target_block)?;
        let (target, keep) = match block {
            // Blocks & ifs break to the end, and take their return params
            Block::If { end, .. } | Block::Block { end, .. } => {
                (end + 1, block.return_params(functions)?)
            }
            // Loops break back to the start, and take their call params
            Block::Loop { start, .. } => (start + 1, block.call_params(functions)?),
        };

        let frame = validator
            .get_control_frame(depth as usize)
            .ok_or_else(|| anyhow!("target control frame doesn't exist"))?;

        let keep = keep.map(|v| val_types_to_size(v)).unwrap_or(0) as u32;
        let total = validator.operand_stack_height() as usize;
        let above_frame = total - frame.height;
        let slots: usize = (stack_pops as usize..above_frame)
            .into_iter()
            .map(|depth| {
                let ty = validator.get_operand_type(depth).flatten();
                if ty == Some(ValType::V128) { 2 } else { 1 }
            })
            .sum();

        let drop = slots
            .checked_sub(keep as usize)
            .ok_or_else(|| anyhow!("negative drop depth"))?;

        Ok(BranchResolveResult::Target {
            target: u32::try_from(target)?,
            dk: DropKeep {
                drop: u16::try_from(drop)?,
                keep: u16::try_from(keep)?,
            },
        })
    }
}

impl CompilerFilter for BlockPass {
    fn compile(
        &mut self,
        compiler: &mut Compiler,
        functions: &FunctionManager,
        pc: usize,
        operator: wasmparser::Operator,
    ) -> Result<bool, crate::HaltError> {
        let wackasm_operator = match operator {
            // blocks & loops just open a new block
            Operator::Block { .. } | Operator::Loop { .. } => {
                self.block_stack.push(BlockIndex(pc as u32));

                None
            }
            Operator::If { .. } => {
                let idx = BlockIndex(pc as u32);
                self.block_stack.push(idx);
                let block = compiler.block_manager.get(&idx)?.clone();
                let (_, r#else, end) = block.if_block()?;

                let false_target = r#else.unwrap_or(end) as u32 + 1;

                Some(WackasmOperator::JumpIfEqz {
                    target: false_target,
                })
            }
            Operator::Else => {
                let if_block = self
                    .block_stack
                    .last()
                    .ok_or_else(|| trap!("no block when we get to else"))?;

                let block = compiler.block_manager.get(if_block)?;

                let (_, _, end) = block.if_block()?;

                Some(WackasmOperator::Jump {
                    target: end as u32 + 1,
                })
            }
            Operator::End => match self.block_stack.pop() {
                Some(_) => None,
                None => Some(WackasmOperator::Return),
            },
            // Br
            Operator::Br { relative_depth } => {
                let branch = self.resolve_branch(compiler, functions, relative_depth, 0)?;
                Some(match branch {
                    BranchResolveResult::Unreachable => WackasmOperator::Unreachable,
                    BranchResolveResult::Return => WackasmOperator::Return,
                    BranchResolveResult::Target { target, dk } => match dk.drop == 0 {
                        true => WackasmOperator::Jump { target },
                        false => WackasmOperator::Br {
                            target,
                            drop_keep: dk,
                        },
                    },
                })
            }

            Operator::BrIf { relative_depth } => {
                let branch = self.resolve_branch(compiler, functions, relative_depth, 1)?;
                Some(match branch {
                    BranchResolveResult::Unreachable => WackasmOperator::Unreachable,
                    BranchResolveResult::Return => WackasmOperator::ReturnIf,
                    BranchResolveResult::Target { target, dk } => match dk.drop == 0 {
                        true => WackasmOperator::JumpIf { target },
                        false => WackasmOperator::BrIf {
                            target,
                            drop_keep: dk,
                        },
                    },
                })
            }
            Operator::BrTable { ref targets } => {
                let mut unreachable = matches!(
                    self.resolve_branch(compiler, functions, 0, 1)?,
                    BranchResolveResult::Unreachable
                );

                let mut results: WackasmTableTargets =
                    Vec::with_capacity(targets.len() as usize + 1);
                if !unreachable {
                    for target in targets.targets() {
                        let depth = target?;
                        let branch = self.resolve_branch(compiler, functions, depth, 1)?;
                        results.push(match branch {
                            BranchResolveResult::Unreachable => {
                                unreachable = true;
                                break;
                            }
                            BranchResolveResult::Return => WackasmTableTarget::Return,
                            BranchResolveResult::Target { target, dk } => {
                                WackasmTableTarget::Break { target, dk }
                            }
                        });
                    }

                    // do the same for the default
                    let branch = self.resolve_branch(compiler, functions, targets.default(), 1)?;
                    results.push(match branch {
                        BranchResolveResult::Unreachable => unreachable!(),
                        BranchResolveResult::Return => WackasmTableTarget::Return,
                        BranchResolveResult::Target { target, dk } => {
                            WackasmTableTarget::Break { target, dk }
                        }
                    });
                }

                Some(if !unreachable {
                    WackasmOperator::BrTable {
                        targets: Box::new(results),
                    }
                } else {
                    WackasmOperator::Unreachable
                })
            }
            _ => return Ok(false),
        };

        if let Some(wackasm_operator) = wackasm_operator {
            compiler.func_body.inner.push(wackasm_operator);
        }

        return Ok(true);
    }
}

impl BlockPass {
    pub fn new() -> Self {
        Self {
            block_stack: Vec::new(),
        }
    }
}
