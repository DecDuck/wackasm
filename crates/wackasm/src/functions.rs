use std::collections::HashMap;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    FuncType, FuncValidator, FuncValidatorAllocations, FunctionBody, Imports, RecGroup,
    SectionLimited, TypeRef, ValidatorResources,
};

use crate::{
    HaltError, Value,
    compiler::{RuntimeWackasmOperator, WackasmFunctionBody},
    macros::trap,
    module::{FunctionIndex, FunctionTypeIndex},
    stack::val_types_to_size,
};

pub(crate) const MAX_CALL_DEPTH: usize = 10_000;

// Derived pointers borrow immutable function allocations. Rebuild after loading or
// deserializing bodies; never serialize these pointers or mutate their allocations.
pub(crate) struct FuncInfo {
    pub(crate) code: *const RuntimeWackasmOperator,
    pub(crate) code_len: usize,
    pub(crate) params_slots: usize,
    pub(crate) results_slots: usize,
    pub(crate) local_defaults: *const Value,
    pub(crate) locals_len: usize,
    pub(crate) frame_size: usize,
    pub(crate) signature_id: u32,
}

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
#[serde(try_from = "FunctionManagerData")]
pub(crate) struct FunctionManager {
    pub(crate) function_type_indexes: Vec<FuncType>,
    pub(crate) function_types: Vec<FunctionTypeIndex>,
    pub(crate) function_bodies: Vec<WackasmFunction>,
    #[serde(skip)]
    pub(crate) info: Vec<FuncInfo>,
    #[serde(skip)]
    pub(crate) signature_ids: Vec<u32>,
}

#[derive(Deserialize)]
struct FunctionManagerData {
    function_type_indexes: Vec<FuncType>,
    function_types: Vec<FunctionTypeIndex>,
    function_bodies: Vec<WackasmFunction>,
}

impl TryFrom<FunctionManagerData> for FunctionManager {
    type Error = HaltError;

    fn try_from(data: FunctionManagerData) -> Result<Self, Self::Error> {
        let mut manager = Self {
            function_type_indexes: data.function_type_indexes,
            function_types: data.function_types,
            function_bodies: data.function_bodies,
            info: Vec::new(),
            signature_ids: Vec::new(),
        };
        manager.rebuild_info()?;
        Ok(manager)
    }
}

impl FunctionManager {
    pub fn new() -> Self {
        Self {
            function_type_indexes: Vec::new(),
            function_types: Vec::new(),
            function_bodies: Vec::new(),
            info: Vec::new(),
            signature_ids: Vec::new(),
        }
    }

    pub(crate) fn rebuild_info(&mut self) -> Result<(), HaltError> {
        let mut canonical_signatures = HashMap::new();
        let mut signature_ids = Vec::with_capacity(self.function_type_indexes.len());
        for (index, ty) in self.function_type_indexes.iter().enumerate() {
            let id = u32::try_from(index).map_err(|_| anyhow!("too many function types"))?;
            signature_ids.push(*canonical_signatures.entry(ty).or_insert(id));
        }
        let mut info = Vec::with_capacity(self.function_bodies.len());
        for (index, function) in self.function_bodies.iter().enumerate() {
            let ty = self.function_type_idx(&FunctionIndex(index as u32))?;
            let params_slots = val_types_to_size(ty.params());
            let results_slots = val_types_to_size(ty.results());
            let (code, code_len, local_defaults, locals_len, frame_size) = match function {
                WackasmFunction::Internal(body) => (
                    body.runtime.as_ptr(),
                    body.runtime.len(),
                    body.local_defaults.as_ptr(),
                    body.local_defaults.len(),
                    body.theoretical_max_stack_size() as usize,
                ),
                WackasmFunction::Import { .. } => (
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    params_slots.max(results_slots),
                ),
            };
            info.push(FuncInfo {
                code,
                code_len,
                params_slots,
                results_slots,
                local_defaults,
                locals_len,
                frame_size,
                signature_id: signature_ids[self.function_types[index].0 as usize],
            });
        }
        self.info = info;
        self.signature_ids = signature_ids;
        Ok(())
    }

    pub(crate) fn signature_id(&self, index: FunctionTypeIndex) -> Result<u32, HaltError> {
        self.signature_ids
            .get(index.0 as usize)
            .copied()
            .ok_or_else(|| trap!("no signature for function type index {:?}", index))
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
}
