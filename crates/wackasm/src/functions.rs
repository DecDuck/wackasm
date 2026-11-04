use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use wasmparser::{
    FuncType, FuncValidator, FuncValidatorAllocations, FunctionBody, Imports, RecGroup,
    SectionLimited, TypeRef, ValType, ValidatorResources,
};

use crate::{
    HaltError, HostYield, Value,
    compiler::WackasmFunctionBody,
    imports::{WackasmHostBridge, WackasmImports, WackasmResource},
    macros::trap,
    module::{FunctionFrame, FunctionIndex, FunctionTypeIndex, InternalFunctionFrame},
    pc::ProgramCounter,
    stack::{FrameOperandStack, GlobalStack as _, RuntimeStack as _, val_types_to_size},
};

const MAX_CALL_DEPTH: usize = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WackasmFunction {
    Internal(WackasmFunctionBody),
    Import {
        module: String,
        name: String,
        args: usize,
        returns: usize,
    },
}

impl WackasmFunction {
    pub fn as_internal(&self) -> Result<&WackasmFunctionBody, HaltError> {
        match self {
            WackasmFunction::Internal(wackasm_function_body) => Ok(wackasm_function_body),
            _ => Err(anyhow!("expected internal function").into()),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FunctionManager {
    pub(crate) function_type_indexes: Vec<FuncType>,
    pub(crate) function_types: Vec<FunctionTypeIndex>,
    pub(crate) function_bodies: Vec<WackasmFunction>,
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

    pub(crate) fn get_function(
        &self,
        function_index: &FunctionIndex,
    ) -> Result<&WackasmFunction, HaltError> {
        Ok(self
            .function_bodies
            .get(function_index.0 as usize)
            .ok_or_else(|| anyhow!("internally inconsistent function index"))?)
    }

    fn get_next_function_index(&self) -> FunctionIndex {
        FunctionIndex(self.function_bodies.len() as u32)
    }

    pub(crate) fn process_import_section(
        &mut self,
        imports: SectionLimited<Imports>,
    ) -> Result<(), HaltError> {
        for import in imports {
            let import = import?;
            match import {
                Imports::Single(_, import) => {
                    let func_type_index = match import.ty {
                        TypeRef::Func(func) => func,
                        _ => Err(anyhow!("non-function imports unsupported"))?,
                    };
                    let func_type =
                        self.function_type_idx_type(&FunctionTypeIndex(func_type_index))?;

                    let args = val_types_to_size(func_type.params());
                    let returns = val_types_to_size(func_type.results());
                    self.function_bodies.push(WackasmFunction::Import {
                        module: import.module.to_owned(),
                        name: import.name.to_owned(),
                        args,
                        returns,
                    });
                    self.function_types.push(FunctionTypeIndex(func_type_index));
                }
                Imports::Compact1 { module, items } => {
                    for import in items {
                        let import = import?;
                        let func_type_index = match import.ty {
                            TypeRef::Func(func) => func,
                            _ => Err(anyhow!("non-function imports unsupported"))?,
                        };

                        let func_type =
                            self.function_type_idx_type(&FunctionTypeIndex(func_type_index))?;

                        let args = val_types_to_size(func_type.params());
                        let returns = val_types_to_size(func_type.results());

                        self.function_bodies.push(WackasmFunction::Import {
                            module: module.to_owned(),
                            name: import.name.to_owned(),
                            args,
                            returns,
                        });
                        self.function_types.push(FunctionTypeIndex(func_type_index));
                    }
                }
                Imports::Compact2 { module, ty, names } => {
                    let func_type_index = match ty {
                        TypeRef::Func(func) => func,
                        _ => Err(anyhow!("non-function imports unsupported"))?,
                    };

                    let func_type =
                        self.function_type_idx_type(&FunctionTypeIndex(func_type_index))?;

                    let args = val_types_to_size(func_type.params());
                    let returns = val_types_to_size(func_type.results());

                    for name in names {
                        let name = name?;

                        self.function_types.push(FunctionTypeIndex(func_type_index));
                        self.function_bodies.push(WackasmFunction::Import {
                            module: module.to_owned(),
                            name: name.to_owned(),
                            args,
                            returns,
                        });
                    }
                }
            }
        }

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
        self.function_bodies
            .push(WackasmFunction::Internal(function_body));

        Ok(allocs)
    }

    pub(crate) fn call_import_internal(
        &self,
        callstack: &mut Vec<FunctionFrame>,
        stack: &mut FrameOperandStack,
        function_index: FunctionIndex,
        arguments: Option<Vec<Value>>,
        module: &str,
        name: &str,
        args: usize,
        returns: usize,
    ) -> Result<Option<HostYield>, HaltError> {
        let arguments = match arguments {
            Some(args) => args,
            None => stack.pull_results_size(args)?,
        };
        if arguments.len() != args {
            return Err(trap!(
                "unexpected amount of arguments: expected {} vs got {}",
                args,
                arguments.len()
            ));
        }

        callstack.push(FunctionFrame::Import {
            module: module.to_owned(),
            name: name.to_owned(),
            args: arguments.clone(),
            should_return: returns
        });

        Ok(Some(HostYield::ImportCall {
            module: module.to_owned(),
            name: name.to_owned(),
            args: arguments,
            should_return: returns,
        }))
    }

    #[must_use]
    pub(crate) fn call_internal(
        &self,
        callstack: &mut Vec<FunctionFrame>,
        stack: &mut FrameOperandStack,
        function_index: FunctionIndex,
        arguments: Option<Vec<Value>>,
    ) -> Result<Option<HostYield>, HaltError> {
        if callstack.len() >= MAX_CALL_DEPTH {
            return Err(trap!("call stack exhausted"));
        }

        let code_body = match self.get_function(&function_index)? {
            WackasmFunction::Internal(wackasm_function_body) => wackasm_function_body,
            WackasmFunction::Import {
                module,
                name,
                args,
                returns,
            } => {
                return self.call_import_internal(
                    callstack,
                    stack,
                    function_index,
                    arguments,
                    module,
                    name,
                    *args,
                    *returns,
                );
            }
        };

        if stack.space() < code_body.theoretical_max_stack_size() as usize {
            return Err(trap!("potential stack overflow"));
        }

        if let Some(arguments) = &arguments
            && arguments.len() != code_body.params_slots
        {
            return Err(trap!("invalid arguments len"));
        }

        let mut stack_base = 0;

        if let Some(arguments) = arguments {
            stack_base = stack.len();
            stack.push_all(&arguments);
            stack.push_all(&code_body.local_defaults);
        } else {
            stack_base = stack.len() - code_body.params_slots; // params are already on the stack, we just adopt them
            stack.push_all(&code_body.local_defaults);
        }

        let function = FunctionFrame::Internal(InternalFunctionFrame {
            function_index,
            pc: 0,
            stack_base,
        });

        callstack.push(function);

        Ok(None)
    }
}
