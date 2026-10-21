use std::{collections::HashMap, sync::Arc};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{ExternalKind, FuncType, FunctionBody, Payload, ValType};

use crate::{
    HaltError, Value,
    blocks::{Block, BlockManager, lookahead_find_blocks},
    functions::FunctionManager,
    globals::GlobalManager,
    macros::unsupported,
    memory::MemoryManager,
    operator::OperatorView,
    pc::ProgramCounter,
    stack::{BlockAwareStack, FrameOperandStack, LocalStore},
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
    pub(crate) block_lookup: Arc<BlockManager>,
    pub(crate) locals: LocalStore,
    pub(crate) code: OperatorView,

    pub(crate) stack: FrameOperandStack,
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
        block: &Block,
        idx: &BlockIndex,
    ) -> Result<(), HaltError> {
        let block_params = match block.ty() {
            wasmparser::BlockType::Empty => None,
            wasmparser::BlockType::Type(_) => None,
            wasmparser::BlockType::FuncType(func_type_index) => {
                let function_type_index = FunctionTypeIndex(*func_type_index);
                let function_type = self
                    .functions
                    .function_type_idx_type(&function_type_index)?;

                Some(function_type.params().to_vec())
            }
        };

        operand_stack.push_block(*idx, block_params.as_ref().map(|v| &**v))?;

        Ok(())
    }

    pub fn new(data: &[u8]) -> Result<Self, HaltError> {
        wasmparser::Validator::new().validate_all(data)?;

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
                Payload::CodeSectionEntry(code_section) => {
                    functions.process_function_body_block(code_section)?;
                }
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
                    for import in import {
                        let import = import?;
                        for import in import {
                            let (import_idx, import) = import?;
                        }
                    }
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
