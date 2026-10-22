use std::{collections::HashMap, hash::Hash};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{BlockType, Operator, ValType};

use crate::{
    HaltError,
    functions::FunctionManager,
    macros::unsupported,
    module::{BlockIndex, FunctionTypeIndex},
    operator::OperatorView,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum Block {
    Block {
        start: usize,
        end: usize,
        ty: BlockType,
    },
    Loop {
        start: usize,
        end: usize,
        ty: BlockType,
    },
    If {
        start: usize,
        r#else: Option<usize>,
        end: usize,
        ty: BlockType,
    },
}

impl Block {
    pub fn ty<'a>(&'a self) -> &'a BlockType {
        match self {
            Block::Block { ty, .. } => ty,
            Block::Loop { ty, .. } => ty,
            Block::If { ty, .. } => ty,
        }
    }

    pub fn if_block(&self) -> Result<(usize, Option<usize>, usize), HaltError> {
        match self {
            Block::If {
                start, r#else, end, ..
            } => Ok((*start, r#else.clone(), *end)),
            _ => Err(anyhow!("not an if block").into()),
        }
    }

    pub fn call_params<'a>(
        &'a self,
        functions: &'a FunctionManager,
    ) -> Result<Option<&'a [ValType]>, HaltError> {
        let return_params = match self.ty() {
            wasmparser::BlockType::Empty => None,
            wasmparser::BlockType::Type(_) => None,
            wasmparser::BlockType::FuncType(func_type) => {
                let func_type_index = FunctionTypeIndex(*func_type); // This is right, don't change this.
                let func_type = functions.function_type_idx_type(&func_type_index)?;
                Some(func_type.params())
            }
        };

        Ok(return_params)
    }

    pub fn return_params<'a>(
        &'a self,
        functions: &'a FunctionManager,
    ) -> Result<Option<&'a [ValType]>, HaltError> {
        let return_params = match self.ty() {
            wasmparser::BlockType::Empty => None,
            wasmparser::BlockType::Type(val_type) => Some(std::slice::from_ref(val_type)),
            wasmparser::BlockType::FuncType(func_type) => {
                let func_type_index = FunctionTypeIndex(*func_type); // This is right, don't change this.
                let func_type = functions.function_type_idx_type(&func_type_index)?;
                Some(&*func_type.results())
            }
        };

        Ok(return_params)
    }
}

pub(crate) enum PartialBlock {
    Block {
        start: usize,
        ty: BlockType,
    },
    Loop {
        start: usize,
        ty: BlockType,
    },
    If {
        start: usize,
        r#else: Option<usize>,
        ty: BlockType,
    },
}

impl PartialBlock {
    pub fn finish(self, end: usize) -> Block {
        match self {
            PartialBlock::Block { start, ty } => Block::Block { start, end, ty },
            PartialBlock::Loop { start, ty } => Block::Loop { start, end, ty },
            PartialBlock::If { start, r#else, ty } => Block::If {
                start,
                r#else,
                end,
                ty,
            },
        }
    }

    pub fn start(&self) -> usize {
        match self {
            PartialBlock::Block { start, .. } => *start,
            PartialBlock::Loop { start, .. } => *start,
            PartialBlock::If { start, r#else, .. } => *start,
        }
    }
}

pub(crate) fn lookahead_find_blocks(code: &OperatorView) -> Result<BlockManager, HaltError> {
    let mut results = BlockManager::new();
    let mut stack = Vec::new();
    for (position, operator) in code.read().enumerate() {
        match operator {
            Operator::Block { blockty } => {
                stack.push(PartialBlock::Block {
                    start: position,
                    ty: blockty,
                });
            }
            Operator::Loop { blockty } => {
                stack.push(PartialBlock::Loop {
                    start: position,
                    ty: blockty,
                });
            }
            Operator::If { blockty } => {
                stack.push(PartialBlock::If {
                    start: position,
                    r#else: None,
                    ty: blockty,
                });
            }
            Operator::Else => {
                let mut top = stack
                    .peek_mut()
                    .ok_or_else(|| anyhow!("empty stack when hitting an else"))?;
                match &mut *top {
                    PartialBlock::If { start, r#else, ty } if r#else.is_none() => {
                        *r#else = Some(position);
                    }
                    _ => {
                        return Err(anyhow!(
                            "hit an else while not in an if block or an if block with an existing else"
                        )
                        .into());
                    }
                }
            }
            Operator::End => {
                if let Some(finished_block) = stack.pop() {
                    let start_position = finished_block.start();
                    let finished_block = finished_block.finish(position);
                    results.insert(BlockIndex(start_position as u32), finished_block);
                }
            }
            Operator::Try { .. } => return Err(unsupported!("try not supported")),
            Operator::TryTable { .. } => return Err(unsupported!("try table not supported")),
            _ => {}
        }
    }

    Ok(results)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockManager {
    pub(crate) blocks: Vec<Option<Block>>,
}

impl BlockManager {
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    pub fn get(&self, block_index: &BlockIndex) -> Result<&Block, HaltError> {
        self.blocks
            .get(block_index.0 as usize)
            .flatten_ref()
            .ok_or_else(|| anyhow!("no block for block_index {:?}", block_index).into())
    }

    pub fn insert(&mut self, block_index: BlockIndex, block: Block) {
        let target_length = usize::max((block_index.0 + 1) as usize, self.blocks.len());
        let to_extend = target_length - self.blocks.len();
        if to_extend > 0 {
            self.blocks.resize(target_length, None);
        }

        self.blocks[block_index.0 as usize] = Some(block);
    }

    pub fn len(&self) -> usize {
        self.blocks.iter().filter(|v| v.is_some()).count()
    }
}
