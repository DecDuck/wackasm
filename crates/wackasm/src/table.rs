use std::collections::HashMap;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    Data, DataKind, Element, ElementItems, ElementKind, SectionLimited, Table, TableInit,
};

use crate::{
    HaltError, Value,
    const_expr::resolve_const_expr,
    macros::{trap, unsupported},
    module::{FunctionIndex, TableIndex, TableValue},
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TableManager {
    pub(crate) tables: Vec<Vec<TableValue>>,
}

impl TableManager {
    pub fn new() -> Self {
        Self { tables: Vec::new() }
    }

    pub fn process_table_section(
        &mut self,
        raw_tables: SectionLimited<Table>,
    ) -> Result<(), HaltError> {
        for (index, table) in raw_tables.into_iter().enumerate() {
            let table = table?;
            let index = TableIndex(index as u32);

            let init_value = match table.init {
                TableInit::RefNull => TableValue::NullRef,
                TableInit::Expr(const_expr) => {
                    let value = resolve_const_expr(const_expr)?
                        .into_value_checked()
                        .ok_or_else(|| {
                            anyhow!("values larger than u64 not supported for table init")
                        })?;
                    TableValue::FuncRef(FunctionIndex(value.0 as u32))
                }
            };
            let table_values = vec![init_value; table.ty.initial as usize];
            self.tables.push(table_values);
        }
        Ok(())
    }

    pub fn process_element_section(
        &mut self,
        raw_elements: SectionLimited<Element>,
    ) -> Result<(), HaltError> {
        for element in raw_elements {
            let element = element?;
            match element.kind {
                ElementKind::Active {
                    table_index,
                    offset_expr,
                } => {
                    let table_index = TableIndex(table_index.unwrap_or(0));
                    let offset = resolve_const_expr(offset_expr)?
                        .into_value_checked()
                        .ok_or_else(|| {
                            anyhow!("values larger than u64 not supported for element section")
                        })?;
                    let offset = offset.0 as u32;

                    let table_value = self
                        .tables
                        .get_mut(table_index.0 as usize)
                        .ok_or_else(|| anyhow!("table for index {:?} not found", table_index))?;

                    match element.items {
                        ElementItems::Functions(fn_indexes) => {
                            for (local_offset, fn_index) in fn_indexes.into_iter().enumerate() {
                                let fn_index = fn_index?;
                                let true_offset = (offset as u32 as usize)
                                    .checked_add(local_offset)
                                    .ok_or_else(|| trap!("out of bounds table access"))?;
                                let slot = table_value
                                    .get_mut(true_offset)
                                    .ok_or_else(|| trap!("out of bounds table access"))?;
                                *slot = TableValue::FuncRef(FunctionIndex(fn_index));
                            }
                        }
                        ElementItems::Expressions(..) => {
                            return Err(unsupported!("element expressions are not supported"));
                        }
                    }
                }

                ElementKind::Passive => {
                    return Err(unsupported!("passive element segments are not supported"));
                }
                ElementKind::Declared => {
                    return Err(unsupported!("declared element segments are not supported"));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, index: &TableIndex) -> Option<&Vec<TableValue>> {
        self.tables.get(index.0 as usize)
    }
}
