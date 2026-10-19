use std::collections::HashMap;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use wasmparser::{
    Data, DataKind, Element, ElementItems, ElementKind, SectionLimited, Table, TableInit,
};

use crate::{
    Value,
    const_expr::resolve_const_expr,
    module::{FunctionIndex, TableIndex, TableValue},
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TableManager {
    pub(crate) tables: HashMap<TableIndex, Vec<TableValue>>,
}

impl TableManager {
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
        }
    }

    pub fn process_table_section(
        &mut self,
        raw_tables: SectionLimited<Table>,
    ) -> Result<(), anyhow::Error> {
        for (index, table) in raw_tables.into_iter().enumerate() {
            let table = table?;
            let index = TableIndex(index as u32);

            let init_value = match table.init {
                TableInit::RefNull => TableValue::NullRef,
                TableInit::Expr(const_expr) => {
                    let value = resolve_const_expr(const_expr)?;
                    match value {
                        Value::FuncRef(function_index) => TableValue::FuncRef(function_index),
                        _ => {
                            return Err(anyhow!("value {:?} not supported for table init", value));
                        }
                    }
                }
            };
            let table_values = vec![init_value; table.ty.initial as usize];
            self.tables.insert(index, table_values);
        }
        Ok(())
    }

    pub fn process_element_section(
        &mut self,
        raw_elements: SectionLimited<Element>,
    ) -> Result<(), anyhow::Error> {
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
                        Value::I32(value) => value,
                        _ => {
                            return Err(anyhow!(
                                "const value {:?} not supported for element table offset",
                                offset
                            ));
                        }
                    };

                    let table_value = self
                        .tables
                        .get_mut(&table_index)
                        .ok_or_else(|| anyhow!("table for index {:?} not found", table_index))?;

                    match element.items {
                        ElementItems::Functions(fn_indexes) => {
                            for (local_offset, fn_index) in fn_indexes.into_iter().enumerate() {
                                let fn_index = fn_index?;
                                let true_offset = offset as usize + local_offset;
                                table_value[true_offset] =
                                    TableValue::FuncRef(FunctionIndex(fn_index))
                            }
                        }
                        ElementItems::Expressions(ref_type, section_limited) => todo!(),
                    }
                }

                ElementKind::Passive => todo!(),
                ElementKind::Declared => todo!(),
            }
        }
        Ok(())
    }

    pub fn get(&self, index: &TableIndex) -> Option<&Vec<TableValue>> {
        self.tables.get(index)
    }
}
