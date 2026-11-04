use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use wasmparser::{
    FuncType, FuncValidator, FuncValidatorAllocations, FunctionBody, Imports, RecGroup,
    SectionLimited, ValType, ValidatorResources,
};

use crate::{
    HaltError, Value, WackasmModule,
    compiler::WackasmFunctionBody,
    macros::trap,
    module::{FunctionFrame, FunctionIndex, FunctionTypeIndex},
    pc::ProgramCounter,
    stack::{FrameOperandStack, GlobalStack as _, LocalStore, val_types_to_size},
};

const MAX_CALL_DEPTH: usize = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FunctionManager {
    pub(crate) function_type_indexes: Vec<FuncType>,
    pub(crate) function_types: Vec<FunctionTypeIndex>,
    pub(crate) function_bodies: Vec<WackasmFunctionBody>,
    pub(crate) init_function_frames: Vec<FunctionFrame>,
}

impl FunctionManager {
    pub fn new() -> Self {
        Self {
            function_type_indexes: Vec::new(),
            function_types: Vec::new(),
            function_bodies: Vec::new(),
            init_function_frames: Vec::new(),
        }
    }

    pub(crate) fn function_type_idx(
        &self,
        function_index: &FunctionIndex,
    ) -> Result<&FuncType, HaltError> {
        self.function_type_idx_type(
            self.function_types
                .get(function_index.0 as usize)
                .ok_or_else(|| trap!("no function type for function index {:?}", function_index))?,
        )
    }

    pub(crate) fn function_type_idx_type(
        &self,
        function_type_index: &FunctionTypeIndex,
    ) -> Result<&FuncType, HaltError> {
        let function_type = self
            .function_type_indexes
            .get(function_type_index.0 as usize)
            .ok_or_else(|| {
                anyhow!(
                    "no function type for function type index {:?}",
                    function_type_index
                )
            })?;

        Ok(function_type)
    }

    pub(crate) fn get_function_code(&self, function_index: &FunctionIndex) -> Result<&WackasmFunctionBody, HaltError> {
        self.function_bodies.get(function_index.0 as usize).ok_or_else(|| anyhow!("internally inconsistent function index").into())
    }

    fn get_next_function_index(&self) -> FunctionIndex {
        FunctionIndex(self.init_function_frames.len() as u32)
    }

    pub(crate) fn process_import_section(
        &mut self,
        imports: SectionLimited<Imports>,
    ) -> Result<(), HaltError> {
        Ok(())
    }

    pub(crate) fn process_type_section(
        &mut self,
        types: SectionLimited<RecGroup>,
    ) -> Result<(), HaltError> {
        let type_groups = types
            .into_iter_with_offsets()
            .map(|entry| entry.map(|(_, group)| group.into_types().collect::<Vec<_>>()))
            .collect::<Result<Vec<_>, _>>()?;

        for sub_type in type_groups.into_iter().flatten() {
            self.function_type_indexes
                .push(sub_type.unwrap_func().clone());
        }

        Ok(())
    }

    pub(crate) fn process_function_block(
        &mut self,
        functions: SectionLimited<u32>,
    ) -> Result<(), HaltError> {
        let functions = functions.into_iter().collect::<Result<Vec<_>, _>>()?;
        for function_type in functions {
            self.function_types.push(FunctionTypeIndex(function_type));
        }

        Ok(())
    }

    pub(crate) fn process_function_body_block(
        &mut self,
        code_section: FunctionBody<'_>,
        func_validator: FuncValidator<ValidatorResources>,
    ) -> Result<FuncValidatorAllocations, HaltError> {
        let mut function_state = FunctionFrame {
            // Stuff that needs to be initialised
            function_index: FunctionIndex(u32::MAX),
            locals: LocalStore::new(),

            // Runtime state
            pc: ProgramCounter::default(),
            stack_base: 0,
        };

        // Check function signature and create parameter locals
        let function_index = &self.get_next_function_index();
        function_state.function_index = *function_index;
        let function_type_index = self
            .function_types
            .get(function_index.0 as usize)
            .ok_or_else(|| anyhow!("no function type for index {:?}", function_index))?;
        let function_type = self
            .function_type_indexes
            .get(function_type_index.0 as usize)
            .ok_or_else(|| {
                anyhow!(
                    "no concrete type for function type index {:?}",
                    function_type_index
                )
            })?;
        let params = function_type.params();
        for param in params {
            Self::declare_local(&mut function_state.locals, param);
        }

        // Create locals
        let locals = code_section.get_locals_reader()?;
        for local in locals {
            let (count, val_type) = local?;
            for _ in 0..count {
                Self::declare_local(&mut function_state.locals, &val_type);
            }
        }

        // Save function body for later
        let (function_body, allocs) =
            WackasmFunctionBody::compile(&code_section, func_validator, self)?;
        self.function_bodies.push(function_body);

        self.init_function_frames.push(function_state);
        Ok(allocs)
    }

    fn declare_local(locals: &mut LocalStore, val_type: &ValType) {
        let index = locals.len() as u32;
        match val_type {
            ValType::V128 => locals.insert_wide(index, 0),
            _ => locals.insert(index, WackasmModule::val_type_to_value(val_type)),
        }
    }

    pub(crate) fn call_internal(
        &mut self,
        callstack: &mut Vec<FunctionFrame>,
        stack: &mut FrameOperandStack,
        function_index: FunctionIndex,
        arguments: Option<Vec<Value>>,
    ) -> Result<(), HaltError> {
        if callstack.len() >= MAX_CALL_DEPTH {
            return Err(trap!("call stack exhausted"));
        }

        let function = self
            .init_function_frames
            .get(function_index.0 as usize)
            .ok_or_else(|| {
                anyhow!("internal module corruption: cannot lookup initial function state")
            })?;

        if stack.space() < self.get_function_code(&function.function_index)?.theoretical_max_stack_size() as usize {
            return Err(trap!("potential stack overflow"));
        }

        let mut function = function.clone();

        // Get function signature
        let function_type_index = self
            .function_types
            .get(function_index.0 as usize)
            .ok_or_else(|| {
                anyhow!(
                    "no function type index for function index {:?}",
                    function_index
                )
            })?;
        let function_type = self
            .function_type_indexes
            .get(function_type_index.0 as usize)
            .ok_or_else(|| {
                anyhow!(
                    "no function type for function type index {:?}",
                    function_type_index
                )
            })?;

        let params = function_type.params();
        let arguments = if let Some(arguments) = arguments {
            let expected_slots = val_types_to_size(params);
            if arguments.len() != expected_slots {
                return Err(HaltError::Error(anyhow!(
                    "invalid amount of argument slots, expected {}, got {}",
                    expected_slots,
                    arguments.len()
                )));
            }
            arguments
        } else {
            let calling_operands_types = function_type.params();
            let arguments = stack.pull_results(calling_operands_types)?;
            arguments
        };

        // Insert arguments
        let mut arguments = arguments.into_iter();
        for (index, val_type) in params.into_iter().enumerate() {
            match val_type {
                wasmparser::ValType::V128 => {
                    let high = arguments.next().unwrap().0 as u128;
                    let low = arguments.next().unwrap().0 as u128;
                    function.locals.set_wide(index as u32, (high << 64) | low)?;
                }
                _ => {
                    let value = arguments.next().unwrap();
                    function.locals.set(index as u32, value)?;
                }
            };
        }

        function.stack_base = stack.len();
        callstack.push(function);

        Ok(())
    }
}
