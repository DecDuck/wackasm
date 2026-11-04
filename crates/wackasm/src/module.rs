use std::{collections::HashMap, rc::Rc, sync::Arc};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{ExternalKind, FuncType, FuncValidatorAllocations, FunctionBody, Payload, ValType, ValidPayload};

use crate::{
    HaltError, Value,
    blocks::{Block, BlockManager},
    compiler::{BlockParams, WackasmFunctionBody},
    executor::FunctionScopedResult,
    functions::FunctionManager,
    globals::GlobalManager,
    macros::unsupported,
    memory::MemoryManager,
    pc::ProgramCounter,
    stack::{FrameOperandStack, GlobalStack, LocalStore, StackSection},
    table::TableManager,
};

macro_rules! type_indexes {
    ($($x:ident),* $(,)?) => {
        $(
        #[repr(transparent)]
        #[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
        pub struct $x(pub(crate) u32);
        )*
    };
}

type_indexes!(
    FunctionTypeIndex,
    FunctionIndex,
    TableIndex,
    MemoryIndex,
    GlobalIndex,
    TagIndex,
    BlockIndex,
);

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ExportIndex {
    Func(FunctionIndex),
    Table(TableIndex),
    Memory(MemoryIndex),
    Global(GlobalIndex),
    Tag(TagIndex),
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub(crate) enum TableValue {
    NullRef,
    FuncRef(FunctionIndex),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FunctionFrame {
    pub(crate) function_index: FunctionIndex,
    pub(crate) locals: LocalStore,
    pub(crate) code: Rc<WackasmFunctionBody>,
    pub(crate) pc: ProgramCounter,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WackasmModule {
    pub(crate) exports: HashMap<String, ExportIndex>,
    pub(crate) tables: TableManager,
    pub(crate) memories: MemoryManager,
    pub(crate) globals: GlobalManager,
    pub(crate) start_func: Option<FunctionIndex>,
    pub(crate) functions: FunctionManager,
}

impl WackasmModule {
    pub fn val_type_to_value(val_type: &ValType) -> Value {
        match val_type {
            ValType::Ref(_) => Value::NULL_REF,
            _ => Value(0),
        }
    }

    pub(crate) fn start_block(
        &mut self,
        operand_stack: &mut FrameOperandStack,
        params: &BlockParams,
        idx: &BlockIndex,
    ) -> Result<(), HaltError> {
        let block_params = match params.get() {
            None => None,
            Some(func_type_index) => {
                let function_type = self.functions.function_type_idx_type(&func_type_index)?;

                Some(function_type.params())
            }
        };

        operand_stack.push_section(StackSection::Block(*idx), block_params)?;

        Ok(())
    }
    
    pub fn new(data: &[u8]) -> Result<Self, HaltError> {
        let mut validator = wasmparser::Validator::new();
        let mut allocs = FuncValidatorAllocations::default();

        let payloads = wasmparser::Parser::new(0)
            .parse_all(data)
            .collect::<Result<Vec<_>, _>>()?;

        let mut exports = HashMap::new();

        let mut tables = TableManager::new();
        let mut memories = MemoryManager::new();
        let mut globals = GlobalManager::new();
        let mut functions = FunctionManager::new();

        let mut start_func = None;

        for payload in payloads {
            if let ValidPayload::Func(func, body) = validator.payload(&payload)? {
                let func_validator = func.into_validator(std::mem::take(&mut allocs));
                allocs = functions.process_function_body_block(body, func_validator)?;
                continue;
            }

            match payload {
                Payload::Version { .. } => {
                    // todo
                }
                Payload::StartSection { func, .. } => {
                    start_func = Some(FunctionIndex(func));
                }
                Payload::TypeSection(types) => {
                    functions.process_type_section(types)?;
                }
                Payload::FunctionSection(funcs) => {
                    functions.process_function_block(funcs)?;
                }
                Payload::TableSection(raw_tables) => {
                    tables.process_table_section(raw_tables)?;
                }
                Payload::ElementSection(raw_elements) => {
                    tables.process_element_section(raw_elements)?;
                }
                Payload::ExportSection(raw_exports) => {
                    let raw_exports = raw_exports.into_iter().collect::<Result<Vec<_>, _>>()?;
                    for export in raw_exports {
                        exports.insert(
                            export.name.to_owned(),
                            match export.kind {
                                ExternalKind::Func => {
                                    ExportIndex::Func(FunctionIndex(export.index))
                                }
                                ExternalKind::Table => ExportIndex::Table(TableIndex(export.index)),
                                ExternalKind::Memory => {
                                    ExportIndex::Memory(MemoryIndex(export.index))
                                }
                                ExternalKind::Global => {
                                    ExportIndex::Global(GlobalIndex(export.index))
                                }
                                ExternalKind::Tag => ExportIndex::Tag(TagIndex(export.index)),
                                ExternalKind::FuncExact => {
                                    ExportIndex::Func(FunctionIndex(export.index))
                                }
                            },
                        );
                    }
                }
                Payload::CodeSectionStart {
                    count: _,
                    unchecked_range: _,
                    size: _,
                } => {} // noop because we don't need to do anything
                Payload::CodeSectionEntry(_) => unreachable!("handled via ValidPayload::Func"),
                Payload::MemorySection(raw_memories) => {
                    memories.process_memory_section(raw_memories)?;
                }
                Payload::GlobalSection(raw_globals) => {
                    globals.process_globals_section(raw_globals)?;
                }
                Payload::DataSection(raw_data) => {
                    memories.process_data_section(raw_data)?;
                }
                Payload::ImportSection(import) => {
                    functions.process_import_section(import)?;
                }
                Payload::CustomSection(_) | Payload::End(_) => {}
                _ => return Err(unsupported!("unsupported section: {:?}", payload)),
            }
        }

        Ok(Self {
            functions,
            exports,
            tables,
            memories,
            globals,
            start_func,
        })
    }
}
