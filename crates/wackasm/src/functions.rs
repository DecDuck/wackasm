use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use wasmparser::{
    FuncType, FuncValidator, FuncValidatorAllocations, FunctionBody, Imports, RecGroup,
    SectionLimited, ValType, ValidatorResources,
};

use crate::{
    HaltError, Value,
    compiler::WackasmFunctionBody,
    macros::trap,
    module::{FunctionFrame, FunctionIndex, FunctionTypeIndex},
    pc::ProgramCounter,
    stack::{FrameOperandStack, GlobalStack as _},
};

const MAX_CALL_DEPTH: usize = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FunctionManager {
    pub(crate) function_type_indexes: Vec<FuncType>,
    pub(crate) function_types: Vec<FunctionTypeIndex>,
    pub(crate) function_bodies: Vec<WackasmFunctionBody>,
}

impl FunctionManager {
    pub fn new() -> Self {
        Self {
            function_type_indexes: Vec::new(),
            function_types: Vec::new(),
            function_bodies: Vec::new(),
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

    pub(crate) fn get_function_code(
        &self,
        function_index: &FunctionIndex,
    ) -> Result<&WackasmFunctionBody, HaltError> {
        self.function_bodies
            .get(function_index.0 as usize)
            .ok_or_else(|| anyhow!("internally inconsistent function index").into())
    }

    fn get_next_function_index(&self) -> FunctionIndex {
        FunctionIndex(self.function_bodies.len() as u32)
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
        // Check function signature and create parameter locals
        let function_index = &self.get_next_function_index();
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

        // Create locals
        let mut local_types = Vec::new();
        let locals = code_section.get_locals_reader()?;
        for local in locals {
            let (count, val_type) = local?;
            for _ in 0..count {
                local_types.push(val_type);
            }
        }

        // Save function body for later
        let (function_body, allocs) =
            WackasmFunctionBody::compile(&code_section, func_validator, self, local_types, params)?;
        self.function_bodies.push(function_body);

        Ok(allocs)
    }

    pub(crate) fn call_internal(
        &self,
        callstack: &mut Vec<FunctionFrame>,
        stack: &mut FrameOperandStack,
        function_index: FunctionIndex,
        arguments: Option<Vec<Value>>,
    ) -> Result<(), HaltError> {
        if callstack.len() >= MAX_CALL_DEPTH {
            return Err(trap!("call stack exhausted"));
        }

        let code_body = self.get_function_code(&function_index)?;

        if stack.space() < code_body.theoretical_max_stack_size() as usize {
            return Err(trap!("potential stack overflow"));
        }

        if let Some(arguments) = &arguments && arguments.len() != code_body.params_slots {
            return Err(trap!("invalid arguments len"));
        }

        let mut function = FunctionFrame {
            function_index,
            pc: 0,
            stack_base: 0,
        };

        if let Some(arguments) = arguments {
            function.stack_base = stack.len();
            stack.push_all(&arguments);
            stack.push_all(&code_body.local_defaults);
        } else {
            function.stack_base = stack.len() - code_body.params_slots; // params are already on the stack, we just adopt them
            stack.push_all(&code_body.local_defaults);
        }

        callstack.push(function);

        Ok(())
    }
}
