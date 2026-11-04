use std::{collections::HashMap, sync::Arc};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    ElementItems, ElementKind, ExternalKind, FuncType, FunctionBody, Operator, Payload, SubType,
    TableInit, ValType,
};

use crate::{
    Value,
    blocks::{Block, lookahead_find_blocks},
    const_expr::{ConstValue, resolve_const_expr},
    operator::OperatorView,
    pc::ProgramCounter,
    stack::{BlockAwareOperandStack, LocalStore, OperandStack, OperandStackFns},
};

macro_rules! type_indexes {
    ($($x:ident),* $(,)?) => {
        $(
        #[repr(transparent)]
        #[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
        pub(crate) struct $x(pub(crate) u32);
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
    pub(crate) block_lookup: Arc<HashMap<BlockIndex, Block>>,
    pub(crate) locals: LocalStore,
    pub(crate) code: OperatorView,

    pub(crate) operand_stack: BlockAwareOperandStack,
    pub(crate) pc: ProgramCounter,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WackasmModule {
    pub(crate) function_type_indexes: HashMap<FunctionTypeIndex, FuncType>,
    pub(crate) function_types: HashMap<FunctionIndex, FunctionTypeIndex>,
    pub(crate) exports: HashMap<String, ExportIndex>,
    pub(crate) init_function_frames: HashMap<FunctionIndex, FunctionFrame>,
    pub(crate) tables: HashMap<TableIndex, Vec<TableValue>>,
}

impl WackasmModule {
    fn val_type_to_value(val_type: &ValType) -> Value {
        match val_type {
            wasmparser::ValType::I32 => Value::I32(i32::default()),
            wasmparser::ValType::I64 => Value::I64(i64::default()),
            wasmparser::ValType::F32 => Value::F32(f32::default()),
            wasmparser::ValType::F64 => Value::F64(f64::default()),
            wasmparser::ValType::V128 => Value::V128(u128::default()),
            wasmparser::ValType::Ref(ref_type) => Value::Ref(0),
        }
    }

    fn parse_function_block(
        code_section: FunctionBody<'_>,
        initial_function_states: &mut Vec<FunctionFrame>,
        function_type_indexes: &HashMap<FunctionTypeIndex, FuncType>,
        function_types: &HashMap<FunctionIndex, FunctionTypeIndex>,
    ) -> Result<(), anyhow::Error> {
        let mut function_state = FunctionFrame {
            // Stuff that needs to be initialised
            function_index: FunctionIndex(u32::MAX),
            locals: LocalStore::new(),
            code: OperatorView::empty(),
            block_lookup: Arc::new(HashMap::new()),

            // Runtime state
            operand_stack: BlockAwareOperandStack::new(0),
            pc: ProgramCounter::new(),
        };

        // Check function signature and create parameter locals
        let function_index = &FunctionIndex(initial_function_states.len() as u32);
        function_state.function_index = *function_index;
        let function_type_index = function_types
            .get(function_index)
            .ok_or(anyhow!("no function type for index {:?}", function_index))?;
        let function_type = function_type_indexes
            .get(function_type_index)
            .ok_or(anyhow!(
                "no concrete type for function type index {:?}",
                function_type_index
            ))?;
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
        function_state.operand_stack =
            BlockAwareOperandStack::new(function_state.block_lookup.len());

        initial_function_states.push(function_state);
        Ok(())
    }

    pub(crate) fn start_block(
        &mut self,
        operand_stack: &mut BlockAwareOperandStack,
        block: &Block,
        idx: &BlockIndex,
    ) -> Result<(), anyhow::Error> {
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
    ) -> Result<&FuncType, anyhow::Error> {
        let function_type_index = self.function_types.get(function_index).ok_or(anyhow!(
            "no function type index for function index {:?}",
            function_index
        ))?;
        let function_type = self
            .function_type_indexes
            .get(function_type_index)
            .ok_or(anyhow!(
                "no function type for function type index {:?}",
                function_type_index
            ))?;

        Ok(function_type)
    }

    pub(crate) fn function_type_idx_type(
        &self,
        function_type_index: &FunctionTypeIndex,
    ) -> Result<&FuncType, anyhow::Error> {
        let function_type = self
            .function_type_indexes
            .get(function_type_index)
            .ok_or(anyhow!(
                "no function type for function type index {:?}",
                function_type_index
            ))?;

        Ok(function_type)
    }

    pub fn new(data: &[u8]) -> Result<Self, anyhow::Error> {
        let payloads = wasmparser::Parser::new(0)
            .parse_all(data)
            .collect::<Result<Vec<_>, _>>()?;

        let mut function_type_indexes = HashMap::new();
        let mut function_types = HashMap::new();
        let mut exports = HashMap::new();
        let mut init_function_frames = Vec::new();
        let mut tables = HashMap::new();

        for payload in payloads {
            match payload {
                Payload::Version {
                    num,
                    encoding,
                    range,
                } => {
                    println!("version {num:?} {encoding:?} {range:?}")
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
                    for (index, table) in raw_tables.into_iter().enumerate() {
                        let table = table?;
                        let index = TableIndex(index as u32);

                        let init_value = match table.init {
                            TableInit::RefNull => TableValue::NullRef,
                            TableInit::Expr(const_expr) => {
                                let value = resolve_const_expr(const_expr)?;
                                match value {
                                    ConstValue::FuncRef(function_index) => {
                                        TableValue::FuncRef(function_index)
                                    }
                                    _ => {
                                        return Err(anyhow!(
                                            "value {:?} not supported for table init",
                                            value
                                        ));
                                    }
                                }
                            }
                        };
                        let table_values = vec![init_value; table.ty.initial as usize];
                        tables.insert(index, table_values);
                    }
                }
                Payload::ElementSection(raw_elements) => {
                    for element in raw_elements {
                        let element = element?;
                        match element.kind {
                            ElementKind::Active {
                                table_index,
                                offset_expr,
                            } => {
                                let table_index = TableIndex(table_index.unwrap_or(0));
                                let offset = resolve_const_expr(offset_expr)?;
                                let offset = match offset {
                                    ConstValue::ConstI32(value) => value,
                                    _ => {
                                        return Err(anyhow!(
                                            "const value {:?} not supported for element table offset",
                                            offset
                                        ));
                                    }
                                };

                                let table_value = tables.get_mut(&table_index).ok_or(anyhow!(
                                    "table for index {:?} not found",
                                    table_index
                                ))?;

                                match element.items {
                                    ElementItems::Functions(fn_indexes) => {
                                        for (local_offset, fn_index) in fn_indexes.into_iter().enumerate() {
                                            let fn_index = fn_index?;
                                            let true_offset = offset as usize + local_offset;
                                            table_value[true_offset] = TableValue::FuncRef(FunctionIndex(fn_index))
                                        }
                                    },
                                    ElementItems::Expressions(ref_type, section_limited) => todo!(),
                                }
                            }

                            ElementKind::Passive => todo!(),
                            ElementKind::Declared => todo!(),
                        }
                    }
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
                Payload::CustomSection(_) | Payload::End(_) => {}
                _ => return Err(anyhow!("unsupported section: {:?}", payload)),
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
        })
    }
}
