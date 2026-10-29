use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use anyhow::anyhow;
use wasmparser::{
    FuncValidator, FuncValidatorAllocations, FunctionBody, Operator, OperatorsReader, ValType,
    ValidatorResources,
};

use crate::{
    HaltError, Value,
    blocks::{BlockManager, lookahead_find_blocks},
    compiler::{
        RuntimeWackasmOperator, WackasmFunctionBody, WackasmOperator, WackasmTableTarget,
        filter::{CompilerFilter, CompilerFilterConcrete},
        filters::{block::BlockPass, locals::LocalsPass, stateless_base::StatelessBasePass},
    },
    functions::FunctionManager,
    stack::{val_type_to_size, val_types_to_size},
};

pub(crate) struct Compiler {
    // Outputs
    pub(crate) func_body: WackasmFunctionBody,

    // Utils
    pub(crate) block_manager: BlockManager,
    pub(crate) func_validator: FuncValidator<ValidatorResources>,

    // Internal state
    pub(crate) transform_pc_lookup: Vec<usize>, // map of old pc to new pc, think of it as a hashmap
    pub(crate) locals_slot_to_index: Vec<usize>,
}

impl Compiler {
    pub fn new(func_validator: FuncValidator<ValidatorResources>) -> Self {
        Self {
            func_body: WackasmFunctionBody::empty(),
            block_manager: BlockManager::new(),
            func_validator,
            transform_pc_lookup: Vec::new(),
            locals_slot_to_index: Vec::new(),
        }
    }

    pub fn create_locals(&mut self, params: &[ValType], local_types: Vec<ValType>) {
        let mut slot_idx = 0;

        // Process arguments/params first
        self.func_body.params_slots = val_types_to_size(params);
        for param_type in params {
            self.locals_slot_to_index.push(slot_idx);
            slot_idx += val_type_to_size(param_type);
        }

        let locals_len = val_types_to_size(&local_types);
        let mut locals = vec![Value(0); locals_len].into_boxed_slice();

        for local_type in local_types {
            self.locals_slot_to_index.push(slot_idx);

            // slot_idx - sself.func_body.params_slots gives us the index into declared locals (without params)
            // rather than the locals as a whole
            if matches!(local_type, ValType::Ref(_)) {
                locals[slot_idx - self.func_body.params_slots] = Value::NULL_REF;
            };

            let size = val_type_to_size(&local_type);
            slot_idx += size;
        }

        self.func_body.local_defaults = locals;
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
            self.transform_pc_lookup
                .push(self.func_body.inner.as_ref().unwrap().len());
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
        self.transform_pc_lookup
            .push(self.func_body.inner.as_ref().unwrap().len());

        ops.get_binary_reader()
            .finish_expression(&self.func_validator.visitor(ops.original_position()))?;

        Ok(())
    }

    pub fn target_rewrite(&mut self) -> Result<(), HaltError> {
        for operator in self.func_body.inner.as_mut().unwrap() {
            match operator {
                WackasmOperator::Jump { target }
                | WackasmOperator::JumpIf { target }
                | WackasmOperator::JumpIfEqz { target }
                | WackasmOperator::Br { target, .. }
                | WackasmOperator::BrIf { target, .. } => {
                    *target = *self
                        .transform_pc_lookup
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
                                    .transform_pc_lookup
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

    pub fn finish(mut self) -> Result<(WackasmFunctionBody, FuncValidatorAllocations), HaltError> {
        // Lookup and transform all operators
        let runtime_operators = self
            .func_body
            .inner
            .take()
            .unwrap()
            .into_iter()
            .map(|v| RuntimeWackasmOperator::from(v))
            .collect();
        self.func_body.runtime = runtime_operators;
        Ok((self.func_body, self.func_validator.into_allocations()))
    }
}
