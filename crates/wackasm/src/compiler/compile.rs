use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use anyhow::anyhow;
use wasmparser::{
    FuncValidator, FuncValidatorAllocations, FunctionBody, Operator, OperatorsReader,
    ValidatorResources,
};

use crate::{
    HaltError,
    blocks::{BlockManager, lookahead_find_blocks},
    compiler::{
        WackasmFunctionBody, WackasmOperator, WackasmTableTarget,
        filter::{CompilerFilter, CompilerFilterConcrete},
        filters::{block::BlockPass, locals::LocalsPass, stateless_base::StatelessBasePass},
    },
    functions::FunctionManager,
};

pub(crate) struct Compiler {
    // Outputs
    pub(crate) func_body: WackasmFunctionBody,

    // Utils
    pub(crate) block_manager: BlockManager,
    pub(crate) func_validator: FuncValidator<ValidatorResources>,

    // Internal state
    pub(crate) compilation_lookup: Vec<usize>, // map of old pc to new pc, think of it as a hashmap
}

impl Compiler {
    pub fn new(func_validator: FuncValidator<ValidatorResources>) -> Self {
        Self {
            func_body: WackasmFunctionBody::empty(),
            block_manager: BlockManager::new(),
            func_validator,
            compilation_lookup: Vec::new(),
        }
    }

    pub fn create_compiler_filters(&self) -> Vec<CompilerFilterConcrete> {
        vec![
            CompilerFilterConcrete::BlockPass(BlockPass::new()),
            CompilerFilterConcrete::LocalsPass(LocalsPass::new()),
            CompilerFilterConcrete::StatelessBase(StatelessBasePass::new()),
        ]
    }

    pub fn block_lookahead(&mut self, code: &FunctionBody) -> Result<(), HaltError> {
        self.block_manager = lookahead_find_blocks(&code)?;
        Ok(())
    }

    pub fn transform(
        &mut self,
        code: &FunctionBody,
        functions: &FunctionManager,
    ) -> Result<(), HaltError> {
        let mut reader = code.get_binary_reader();
        self.func_validator.read_locals(&mut reader)?; // declare locals, advance past them
        let mut ops = OperatorsReader::new(reader);

        let mut compiler_passes = self.create_compiler_filters();

        let mut pc = 0;
        while !ops.eof() {
            self.compilation_lookup.push(self.func_body.inner.len());
            let (operator, offset) = ops.read_with_offset()?;
            for filter in &mut compiler_passes {
                if filter.compile(&mut *self, functions, pc, operator.clone())? {
                    break;
                }
            }
            self.func_validator.op(offset, &operator)?; // advance the validator after the filters
            self.func_body.max_stack_operands = self
                .func_body
                .max_stack_operands
                .max(self.func_validator.operand_stack_height());
            pc += 1;
        }
        self.compilation_lookup.push(self.func_body.inner.len());

        ops.get_binary_reader()
            .finish_expression(&self.func_validator.visitor(ops.original_position()))?;

        Ok(())
    }

    pub fn target_rewrite(&mut self) -> Result<(), HaltError> {
        for operator in &mut self.func_body.inner {
            match operator {
                WackasmOperator::Jump { target }
                | WackasmOperator::JumpIf { target }
                | WackasmOperator::JumpIfEqz { target }
                | WackasmOperator::Br { target, .. }
                | WackasmOperator::BrIf { target, .. } => {
                    *target = *self
                        .compilation_lookup
                        .get(*target as usize)
                        .ok_or_else(|| anyhow!("no new pc for old pc"))?
                        as u32;
                }
                WackasmOperator::BrTable { targets } => {
                    for target in targets.iter_mut() {
                        match target {
                            WackasmTableTarget::Return => {}
                            WackasmTableTarget::Break { target, .. } => {
                                *target = *self
                                    .compilation_lookup
                                    .get(*target as usize)
                                    .ok_or_else(|| anyhow!("no new pc for old pc {}", target))?
                                    as u32;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(WackasmFunctionBody, FuncValidatorAllocations), HaltError> {
        Ok((self.func_body, self.func_validator.into_allocations()))
    }
}
