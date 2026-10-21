use std::{collections::HashMap, sync::Arc};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    ExternalKind, FuncType, FunctionBody, Payload, ValType,
};

use crate::{
    HaltError, Value, blocks::{Block, BlockManager, lookahead_find_blocks}, globals::GlobalManager, macros::unsupported, memory::MemoryManager, operator::OperatorView, pc::ProgramCounter, stack::{BlockAwareStack, FrameOperandStack, LocalStore}, table::TableManager,
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
    pub(crate) function_type_indexes: HashMap<FunctionTypeIndex, FuncType>,
    pub(crate) function_types: HashMap<FunctionIndex, FunctionTypeIndex>,
    pub(crate) exports: HashMap<String, ExportIndex>,
    pub(crate) init_function_frames: HashMap<FunctionIndex, FunctionFrame>,
    pub(crate) tables: TableManager,
    pub(crate) memories: MemoryManager,
    pub(crate) globals: GlobalManager,
    pub(crate) start_func: Option<FunctionIndex>,
}

impl WackasmModule {
    fn val_type_to_value(val_type: &ValType) -> Value {
        match val_type {
            ValType::Ref(_) => Value::NULL_REF,
            _ => Value(0),
        }
    }

    fn parse_function_block(
        code_section: FunctionBody<'_>,
        initial_function_states: &mut Vec<FunctionFrame>,
        function_type_indexes: &HashMap<FunctionTypeIndex, FuncType>,
        function_types: &HashMap<FunctionIndex, FunctionTypeIndex>,
    ) -> Result<(), HaltError> {
        let mut function_state = FunctionFrame {
            // Stuff that needs to be initialised
            function_index: FunctionIndex(u32::MAX),
            locals: LocalStore::new(),
            code: OperatorView::empty(),
            block_lookup: Arc::new(BlockManager::new()),

            // Runtime state
            stack: FrameOperandStack::new(0),
            pc: ProgramCounter::new(),
        };

        // Check function signature and create parameter locals
        let function_index = &FunctionIndex(initial_function_states.len() as u32);
        function_state.function_index = *function_index;
        let function_type_index = function_types
            .get(function_index)
            .ok_or_else(|| anyhow!("no function type for index {:?}", function_index))?;
        let function_type = function_type_indexes
            .get(function_type_index)
            .ok_or_else(|| {
                anyhow!(
                    "no concrete type for function type index {:?}",
                    function_type_index
                )
            })?;
        let params = function_type.params();
        for param in params {
            function_state.locals.insert(
                function_state.locals.len() as u32,
                WackasmModule::val_type_to_value(param),
            );
        }

        // Create locals
        let locals = code_section.get_locals_reader()?;
        for local in locals {
            let (count, val_type) = local?;
            for _ in 0..count {
                function_state.locals.insert(
                    function_state.locals.len() as u32,
                    WackasmModule::val_type_to_value(&val_type),
                );
            }
        }

        // Save function body for later
        function_state.code = OperatorView::new(code_section)?;
        function_state.block_lookup = Arc::new(lookahead_find_blocks(&function_state.code)?);
        // reinit block stack for performance
        function_state.stack = FrameOperandStack::new(function_state.block_lookup.len());

        initial_function_states.push(function_state);
        Ok(())
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
                let function_type = self.function_type_idx_type(&function_type_index)?;

                Some(function_type.params().to_vec())
            }
        };
        let params = if let Some(params) = block_params {
            Some(operand_stack.pull_reversed_from_stack(&params)?)
        } else {
            None
        };
        operand_stack.push_block(*idx, params)?;

        Ok(())
    }

    pub(crate) fn function_idx_type(
        &self,
        function_index: &FunctionIndex,
    ) -> Result<&FuncType, HaltError> {
        let function_type_index = self.function_types.get(function_index).ok_or_else(|| {
            anyhow!(
                "no function type index for function index {:?}",
                function_index
            )
        })?;
        let function_type = self
            .function_type_indexes
            .get(function_type_index)
            .ok_or_else(|| {
                anyhow!(
                    "no function type for function type index {:?}",
                    function_type_index
                )
            })?;

        Ok(function_type)
    }

    pub(crate) fn function_type_idx_type(
        &self,
        function_type_index: &FunctionTypeIndex,
    ) -> Result<&FuncType, HaltError> {
        let function_type = self
            .function_type_indexes
            .get(function_type_index)
            .ok_or_else(|| {
                anyhow!(
                    "no function type for function type index {:?}",
                    function_type_index
                )
            })?;

        Ok(function_type)
    }

    pub fn new(data: &[u8]) -> Result<Self, HaltError> {
        wasmparser::Validator::new().validate_all(data)?;

        let payloads = wasmparser::Parser::new(0)
            .parse_all(data)
            .collect::<Result<Vec<_>, _>>()?;

        let mut function_type_indexes = HashMap::new();
        let mut function_types = HashMap::new();
        let mut exports = HashMap::new();
        let mut init_function_frames = Vec::new();

        let mut tables = TableManager::new();
        let mut memories = MemoryManager::new();
        let mut globals = GlobalManager::new();

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
                    let type_groups = types
                        .into_iter_with_offsets()
                        .map(|entry| entry.map(|(_, group)| group.into_types().collect::<Vec<_>>()))
                        .collect::<Result<Vec<_>, _>>()?;

                    let types = type_groups.into_iter().flatten().enumerate();
                    for (index, sub_type) in types {
                        function_type_indexes.insert(
                            FunctionTypeIndex(index as u32),
                            sub_type.unwrap_func().clone(),
                        );
                    }
                }
                Payload::FunctionSection(functions) => {
                    let functions = functions.into_iter().collect::<Result<Vec<_>, _>>()?;
                    for (function_index, function_type) in functions.into_iter().enumerate() {
                        function_types.insert(
                            FunctionIndex(function_index as u32),
                            FunctionTypeIndex(function_type),
                        );
                    }
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
                    WackasmModule::parse_function_block(
                        code_section,
                        &mut init_function_frames,
                        &function_type_indexes,
                        &function_types,
                    )?;
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

        let init_function_frames = init_function_frames
            .into_iter()
            .enumerate()
            .map(|(index, function)| (FunctionIndex(index as u32), function))
            .collect::<HashMap<_, _>>();

        Ok(Self {
            function_type_indexes,
            function_types,
            exports,
            init_function_frames,
            tables,
            memories,
            globals,
            start_func,
        })
    }
}
